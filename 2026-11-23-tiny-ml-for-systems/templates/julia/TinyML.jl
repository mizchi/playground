"""
小さいモデルを自前で作るための雛形 (Julia 版)。依存は標準ライブラリ (Random) だけ。

ts/src/nn.ts・rust/src/lib.rs・templates/python/tinyml.py と同じ構造:
  - 隠れ層 ReLU、出力 1 ユニット + sigmoid、損失は交差エントロピー (sizes=[d, 1] ならロジスティック回帰)
  - 誤差逆伝搬は手で書く:  δ_L = p - y,  ∂W_l = δ_l a_lᵀ,  δ_{l-1} = (W_lᵀ δ_l) ⊙ [a_{l-1} > 0]
  - Adam、ミニバッチ、数値微分による勾配チェック、AUC、重みの JSON 書き出し

Julia は列優先なので、データ行列は (特徴量の次元, 行数) で持つ (1 列が 1 サンプル)。
"""
module TinyML

using Random

export MLP, fit!, predict, grads, step!, gradcheck, auc, to_json, param_count

mutable struct MLP
    sizes::Vector{Int}
    W::Vector{Matrix{Float64}}   # W[l] は (out, in)
    b::Vector{Vector{Float64}}
    m::Vector{Array{Float64}}    # Adam の 1 次モーメント (W..., b...)
    v::Vector{Array{Float64}}    # Adam の 2 次モーメント
    t::Int
end

function MLP(sizes::Vector{Int}; seed::Integer=1)
    sizes[end] == 1 || error("output size must be 1")
    rng = Xoshiro(seed)
    W = [randn(rng, o, i) .* sqrt(2 / i) for (i, o) in zip(sizes[1:end-1], sizes[2:end])]  # He 初期化
    b = [zeros(o) for o in sizes[2:end]]
    ps = Array{Float64}[W; b]
    MLP(sizes, W, b, [zero(p) for p in ps], [zero(p) for p in ps], 0)
end

param_count(m::MLP) = sum(length, m.W) + sum(length, m.b)

"X: (d, n) → (各層の出力, P(y=1) の n 要素ベクトル)"
function forward(m::MLP, X::AbstractMatrix)
    acts = Matrix{Float64}[X]
    L = length(m.W)
    for l in 1:L
        z = m.W[l] * acts[end] .+ m.b[l]
        push!(acts, l < L ? max.(z, 0) : z)
    end
    p = @. 1 / (1 + exp(-acts[end][1, :]))
    acts, p
end

predict(m::MLP, X::AbstractMatrix) = forward(m, X)[2]
predict(m::MLP, x::AbstractVector) = forward(m, reshape(x, :, 1))[2][1]

"バッチ平均の損失と勾配。逆伝搬の本体"
function grads(m::MLP, X::AbstractMatrix, y::AbstractVector, w::AbstractVector=ones(length(y)))
    n = length(y)
    acts, p = forward(m, X)
    loss = -sum(@. w * (y * log(p + 1e-12) + (1 - y) * log(1 - p + 1e-12))) / n
    δ = reshape((p .- y) .* w ./ n, 1, n)          # δ_L = p - y (sigmoid + 交差エントロピー)
    L = length(m.W)
    gW = Vector{Matrix{Float64}}(undef, L)
    gb = Vector{Vector{Float64}}(undef, L)
    for l in L:-1:1
        gW[l] = δ * acts[l]'
        gb[l] = vec(sum(δ; dims=2))
        if l > 1
            δ = (m.W[l]' * δ) .* (acts[l] .> 0)    # ReLU の微分は 0 か 1
        end
    end
    loss, gW, gb
end

"Adam 1 ステップ"
function step!(m::MLP, gW, gb; lr=3e-3)
    m.t += 1
    c1, c2 = 1 - 0.9^m.t, 1 - 0.999^m.t
    for (i, (p, g)) in enumerate(zip(Array{Float64}[m.W; m.b], Array{Float64}[gW; gb]))
        mi, vi = m.m[i], m.v[i]
        @. mi = 0.9 * mi + 0.1 * g
        @. vi = 0.999 * vi + 0.001 * g * g
        @. p -= lr * (mi / c1) / (sqrt(vi / c2) + 1e-8)
    end
    m
end

function fit!(m::MLP, X::AbstractMatrix, y::AbstractVector; epochs=4, batchsize=128, lr=3e-3, pos_weight=1.0, seed=2)
    rng = Xoshiro(seed)
    n = length(y)
    for _ in 1:epochs
        order = randperm(rng, n)
        for s in 1:batchsize:n
            idx = order[s:min(n, s + batchsize - 1)]
            w = [y[i] > 0.5 ? pos_weight : 1.0 for i in idx]
            _, gW, gb = grads(m, X[:, idx], y[idx], w)
            step!(m, gW, gb; lr)
        end
    end
    m
end

"解析的勾配と数値微分の最大相対誤差。逆伝搬を書き換えたら必ず回す"
function gradcheck(m::MLP, X, y; h=1e-5)
    _, gW, gb = grads(m, X, y)
    worst = 0.0
    for (p, g) in zip(Array{Float64}[m.W; m.b], Array{Float64}[gW; gb])
        for i in eachindex(p)
            orig = p[i]
            p[i] = orig + h; lp = grads(m, X, y)[1]
            p[i] = orig - h; lm = grads(m, X, y)[1]
            p[i] = orig
            num = (lp - lm) / (2h)
            worst = max(worst, abs(num - g[i]) / max(1e-8, abs(num) + abs(g[i])))
        end
    end
    worst
end

"ROC AUC (順位統計)"
function auc(scores::AbstractVector, labels::AbstractVector)
    order = sortperm(scores)
    pos = 0; neg = 0; ranksum = 0
    for (r, i) in enumerate(order)
        if labels[i] > 0.5
            pos += 1; ranksum += r
        else
            neg += 1
        end
    end
    (ranksum - pos * (pos + 1) / 2) / (pos * neg)
end

"TS の MLP.toJSON() と同じ形: W は (out, in) の row-major を平らにしたもの (JSON ライブラリなしで書く)"
function to_json(m::MLP)
    nums(v) = "[" * join(string.(v), ",") * "]"
    "{\"sizes\":" * nums(m.sizes) *
    ",\"W\":[" * join([nums(vec(permutedims(W))) for W in m.W], ",") * "]" *
    ",\"b\":[" * join([nums(b) for b in m.b], ",") * "]}"
end

end # module
