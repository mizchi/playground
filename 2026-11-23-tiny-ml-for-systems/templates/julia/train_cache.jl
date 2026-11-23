# 雛形の使い方の例: キャッシュ admission のモデルを学習し、評価し、重みとスコアを書き出す。
#
#   julia train_cache.jl [../../bench/data]
#
# データ・分け方・書き出し先は templates/python/train_cache.py と同じ。
# スコアは <data>/scores/jl-template-<名前>.f32 に書き、bench/rust の sim がヒット率を測る。
include("TinyML.jl")
using .TinyML

data = abspath(length(ARGS) > 0 ? ARGS[1] : joinpath(@__DIR__, "../../bench/data"))
n, d = 600_000, 11
# ファイルは (行数 × 次元) の row-major。Julia の列優先で読むと、そのまま (次元, 行数) になる
X = Float64.(reshape(reinterpret(Float32, read(joinpath(data, "cache_X.f32"))), d, n))
y = Float64.(reinterpret(Float32, read(joinpath(data, "cache_y.f32"))))
train = n * 3 ÷ 10

out = joinpath(@__DIR__, "out")
mkpath(out)
mkpath(joinpath(data, "scores"))
for (name, sizes) in [("logreg", [d, 1]), ("mlp16", [d, 16, 1])]
    m = MLP(sizes; seed=1)
    t = @elapsed fit!(m, X[:, 1:train], y[1:train]; epochs=4, batchsize=128, lr=3e-3)
    scores = predict(m, X)
    a = auc(scores[train+1:end], y[train+1:end])
    println(rpad(name, 7), " ", lpad(param_count(m), 4), " params  train $(round(t; digits=2))s  test AUC $(round(a; digits=4))")
    write(joinpath(out, "$name.json"), to_json(m))
    write(joinpath(data, "scores", "jl-template-$name.f32"), Float32.(scores))
end
println("wrote out/*.json and $(joinpath(data, "scores"))/jl-template-*.f32")
