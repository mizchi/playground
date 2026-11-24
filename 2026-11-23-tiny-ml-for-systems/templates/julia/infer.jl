# 推論側の使い方 (Julia 版)。どの言語の雛形が書いた重み (JSON) でも読み込んで推論する。
#
#   julia infer.jl <weights.json> [学習側が書いたスコア.f32] [--ttl]
#
# 1 行ずつの推論 (製品の判断と同じ使い方) と、まとめての推論の両方を測る。
include("TinyML.jl")
using .TinyML

ttl = "--ttl" in ARGS
args = filter(!=("--ttl"), ARGS)
isempty(args) && error("usage: julia infer.jl <weights.json> [scores.f32] [--ttl]")
model = from_json(read(args[1], String))
d = model.sizes[1]
data = abspath(joinpath(@__DIR__, "../../bench/data"))
rows = 100_000
X = Float64.(reshape(reinterpret(Float32, read(joinpath(data, "cache_X.f32"))), d, :)[:, 1:rows])

"暖機を 1 回してから n 回測り、一番速い回の秒数と結果を返す (初回のコンパイルを除く)"
function best_of(f, n)
    out = f()
    best = Inf
    for _ in 1:n
        t = @elapsed out = f()
        best = min(best, t)
    end
    best, out
end

sec, one = best_of(3) do
    [predict_logit(model, view(X, :, i)) for i in 1:rows]
end
one_ns = sec * 1e9 / rows
sec, p = best_of(() -> predict(model, X), 3)
batch_ns = sec * 1e9 / rows
@assert all(abs.(1 ./ (1 .+ exp.(-one)) .- p) .< 1e-9) "1-row and batch disagree"
ours = ttl ? exp.(one) : p

parts = splitpath(args[1])
line = "jl infer $(join(parts[max(1, end-2):end], "/")): $rows rows, 1-row $(round(Int, one_ns)) ns/row, batch $(round(Int, batch_ns)) ns/row"
line *= ttl ? ", median TTL $(round(Int, sort(ours)[rows ÷ 2]))" : ", admit $(round(100 * count(>(0.5), p) / rows; digits=1))%"
if length(args) > 1
    theirs = Float64.(reinterpret(Float32, read(args[2])))[1:rows]
    diff = abs.(ours .- theirs)
    ttl && (diff ./= max.(1e-9, abs.(theirs)))
    line *= ", max $(ttl ? "relative " : "")|diff| vs trainer = $(round(maximum(diff); sigdigits=2))"
end
println(line)
println("bench lang=julia task=infer one_row_ns=$(round(Int, one_ns)) batch_ns=$(round(Int, batch_ns))")
