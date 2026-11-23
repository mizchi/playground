# bench/jax/ttl.py・bench/autodiff (Rust std::autodiff) と同じ問題を、Enzyme.jl で解く。
# Rust の std::autodiff と同じ Enzyme (LLVM レベルの自動微分) を使うので、普通のループをそのまま微分できる。
#
#   julia --project=diff -e 'using Pkg; Pkg.instantiate()'
#   julia --project=diff diff/ttl_enzyme.jl [bhr|ohr]
using Enzyme

const D = 11
const H = 16
const NP = H * D + H + H + 1   # W1 (H×D, 行ごと) | b1 | w2 | b2
const TAU = 0.25
const MU = 10.0
const LOG_INF = 30.0

function log_ttl(p, X, i)
    out = p[NP]
    @inbounds for j in 1:H
        z = p[H*D+j]
        for k in 1:D
            z += p[(j-1)*D+k] * X[k, i]
        end
        if z > 0
            out += p[H*D+H+j] * z
        end
    end
    out
end

"(soft なヒット, 平均占有量)。jax の terms() と同じ式"
function terms(p, X, lg, g, z, w)
    n = length(lg)
    hit = 0.0; occ = 0.0
    @inbounds for i in 1:n
        lt = log_ttl(p, X, i)
        hit += w[i] / (1 + exp(-(lt - lg[i]) / TAU))
        occ += z[i] * min(g[i], exp(lt))
    end
    hit, occ / n
end

function loss(p, X, lg, g, z, w, cap)
    hit, occ = terms(p, X, lg, g, z, w)
    over = max(occ / cap - 1, 0.0)
    -hit + MU * over * over
end

field(meta, key) = parse(Float64, match(Regex("\"$key\":([0-9.]+)"), meta)[1])

target = length(ARGS) > 0 ? ARGS[1] : "bhr"
data = abspath(joinpath(@__DIR__, "../../../bench/data"))
meta = read(joinpath(data, "cache.json"), String)
n, tr, cap, h0 = Int(field(meta, "n")), Int(field(meta, "train")), field(meta, "capacity"), field(meta, "H")
X = Float64.(reshape(reinterpret(Float32, read(joinpath(data, "cache_X.f32"))), D, n))
ids = reinterpret(UInt32, read(joinpath(data, "cache_id.u32")))
size = Float64.(reinterpret(UInt32, read(joinpath(data, "cache_size.u32"))))

# 次に同じ id が来るまでの間隔。学習区間の外に出るものは ∞ (未来を見ない)
gap = fill(Inf, n)
last = Dict{UInt32,Int}()
for t in n:-1:1
    nx = get(last, ids[t], 0)
    if nx != 0 && nx <= tr
        gap[t] = nx - t
    end
    last[ids[t]] = t
end
g = gap[1:tr]
lg = [isinf(v) ? LOG_INF : log(v) for v in g]
z = size[1:tr]
w = target == "bhr" ? z ./ sum(z) : fill(1 / tr, tr)
Xtr = X[:, 1:tr]

# jax・Rust 版と同じ決定的な初期値 (添字は 0 始まりの式に合わせる)
p = zeros(NP)
for i in 0:H-1, j in 0:D-1
    p[i*D+j+1] = sin(1.0 + 0.7i + 0.3j) * 0.3
end
p[H*D+1:H*D+H] .= 0.1
p[NP] = log(h0)

lr, steps = 0.01, 500
m = zeros(NP); v = zeros(NP)
t0 = time()
for k in 1:steps
    dp = zeros(NP)
    _, val = Enzyme.autodiff(ReverseWithPrimal, loss, Active,
        Duplicated(p, dp), Const(Xtr), Const(lg), Const(g), Const(z), Const(w), Const(cap))
    c1, c2 = 1 - 0.9^k, 1 - 0.999^k
    @. m = 0.9m + 0.1dp
    @. v = 0.999v + 0.001dp^2
    @. p -= lr * (m / c1) / (sqrt(v / c2) + 1e-8)
    if k in (1, 100, 250, 500)
        hit, occ = terms(p, Xtr, lg, g, z, w)
        println("step $(lpad(k, 3))  loss $(round(val; digits=4))  soft $target $(round(hit; digits=4))  occupancy/capacity $(round(occ / cap; digits=3))")
    end
end
train_s = time() - t0

# バイアスをずらして、学習区間の平均占有量をちょうど容量に合わせる
lo, hi = -20.0, 20.0
b2 = p[NP]
for _ in 1:60
    mid = (lo + hi) / 2
    p[NP] = b2 + mid
    global lo, hi = terms(p, Xtr, lg, g, z, w)[2] < cap ? (mid, hi) : (lo, mid)
end
p[NP] = b2 + lo
hit, occ = terms(p, Xtr, lg, g, z, w)
println("calibrated: soft $target $(round(hit; digits=4))  occupancy/capacity $(round(occ / cap; digits=3))")
println("train $(round(train_s; digits=2))s ($steps steps, full batch $tr rows, 1 thread, 初回のコンパイルを含む)")

# 重みを共通の JSON 形式で書く (TS / Rust の MLP.fromJSON で読み、exp(predictLogit(x)) が TTL)
nums(v) = "[" * join(string.(v), ",") * "]"
outdir = joinpath(@__DIR__, "..", "out")
mkpath(outdir)
write(joinpath(outdir, "ttl-enzyme-$target.json"),
    "{\"sizes\":[$D,$H,1],\"W\":[" * nums(p[1:H*D]) * "," * nums(p[H*D+H+1:H*D+2H]) * "]," *
    "\"b\":[" * nums(p[H*D+1:H*D+H]) * "," * nums([p[NP]]) * "]}")

mkpath(joinpath(data, "ttl"))
write(joinpath(data, "ttl", "jl-template-enzyme-$target.f32"), Float32[exp(log_ttl(p, X, i)) for i in 1:n])
println("wrote ttl/jl-template-enzyme-$target.f32")
