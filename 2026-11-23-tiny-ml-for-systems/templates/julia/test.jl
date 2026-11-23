# 逆伝搬の勾配チェックと XOR。  julia test.jl
include("TinyML.jl")
using .TinyML
using Random

rng = Xoshiro(1)
X = rand(rng, 5, 8) .* 2 .- 1          # (d, n)
y = Float64.(rand(rng, 8) .< 0.5)
m = MLP([5, 7, 4, 1]; seed=1)
err = gradcheck(m, X, y)
println("gradient check: max relative error = $(round(err; sigdigits=3)) ($(param_count(m)) params)")
@assert err < 1e-6 "backprop is wrong"

Xx = [0.0 0 1 1; 0 1 0 1]
yx = [0.0, 1, 1, 0]
xor = fit!(MLP([2, 16, 1]; seed=7), Xx, yx; epochs=2000, batchsize=4, lr=0.02)
out = predict(xor, Xx)
println("XOR: ", join(string.(round.(out; digits=3)), " "), " (expected ~0 1 1 0)")
@assert round.(out) == yx

@assert abs(auc([0.1, 0.4, 0.35, 0.8], [0, 0, 1, 1]) - 0.75) < 1e-12
@assert startswith(to_json(xor), "{\"sizes\":[2,16,1]")
println("OK")
