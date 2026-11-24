"""Rebuild the three worked-example figures from the counts and the certificate."""
from html import escape
from math import cos, exp, isqrt, log, pi, sin
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "explore"))
from three_zero_width import ORDINATES, SCALE, enclose_solution


def label(x, y, value, anchor="start", size=18):
    return (f'<text x="{x:.2f}" y="{y:.2f}" text-anchor="{anchor}" '
            f'font-size="{size}">{escape(str(value))}</text>')


def line(x1, y1, x2, y2, color="#849197"):
    return f'<line x1="{x1:.2f}" y1="{y1:.2f}" x2="{x2:.2f}" y2="{y2:.2f}" stroke="{color}"/>'


def circle(x, y, radius=4):
    return f'<circle cx="{x:.2f}" cy="{y:.2f}" r="{radius}" fill="#006978"/>'


def save(name, height, description, drawing):
    body = "\n".join(drawing)
    svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="480" height="{height}" '
           f'viewBox="0 0 480 {height}" role="img" aria-label="{escape(description)}">\n'
           '<rect width="100%" height="100%" fill="#fff"/>\n'
           f'<g font-family="system-ui,sans-serif" fill="#182e39">\n{body}\n</g></svg>\n')
    (HERE / "figures" / f"{name}.svg").write_text(svg, encoding="utf-8")


def count_error(limit):
    count = sum(all(n % (k*k) for k in range(2, isqrt(n)+1))
                for n in range(1, int(limit)+1))
    return count - 6*limit/pi**2


def observed():
    drawing = [label(16, 27, "実際の個数 − 予想の個数"),
               label(16, 54, "10まで：7 − 6.08 ≈ +0.92", size=16)]
    for level in range(3):
        y = 178 - 46*level
        drawing.extend([label(33, y+5, level, "end", 15), line(43, y, 449, y)])
    for n in range(10, 31):
        x = 49 + (n-10)*19.6
        drawing.append(circle(x, 178 - 46*count_error(n)))
        if n % 5 == 0:
            drawing.append(label(x, 204, n, "middle", 16))
    drawing.append(label(248, 231, "数える上限", "middle", 16))
    save("count-error", 244, "10から30まで整数を一個ずつ増やして求めた個数のずれ", drawing)


def contributions(limits, weights, frequency):
    drawing = [label(16, 28, "第1零点に対応する余弦の例"),
               label(23, 62, "上限", size=16), label(396, 62, "重みを掛けた値", "middle", 16),
               line(243, 76, 243, 205)]
    for i, (v, weight) in enumerate(zip(limits, weights)):
        y = 94 + i*34
        value = weight*cos(frequency*log(v/100))
        end = 243 + value*500
        tint = "#006978" if value > 0 else "#904521"
        drawing.extend([label(22, y+5, f"{v:.2f}"),
                        f'<rect x="{min(243, end):.2f}" y="{y-7}" width="{abs(end-243):.2f}" height="14" fill="{tint}"/>',
                        label(400, y+5, f"{value:+.4f}".replace("-", "−"), "middle")])
    drawing.append(label(243, 230, "0", "middle", 16))
    save("averaging", 244, "四つの上限での寄与。右がプラス、左がマイナス", drawing)


def scores(inner, outer, frequencies, limits):
    c = [cos(b*inner) for b in frequencies]
    d = [-b*sin(b*inner) for b in frequencies]
    coeff = [d[(j+1)%3]*c[(j+2)%3]-d[(j+2)%3]*c[(j+1)%3] for j in range(3)]
    def score(v):
        return sum(a*cos(b*(log(v/100)-outer)) for a,b in zip(coeff, frequencies))
    def pos(v):
        return 53 + 385*(v-100)/(limits[-1]-100)
    vals = [100+(limits[-1]-100)*j/500 for j in range(501)]
    scale = 105/max(map(score, vals))
    points = " ".join(f"{pos(v):.2f},{169-scale*score(v):.2f}" for v in vals)
    drawing = [label(16, 30, "証明に使う点数"), line(52, 169, 450, 169),
               label(43, 174, "0点", "end", 15),
               f'<polyline fill="none" stroke="#006978" stroke-width="2.8" points="{points}"/>']
    for v in limits:
        drawing.extend([circle(pos(v),169,4.5), label(pos(v),198, f"{v:.2f}".removesuffix(".00"), "middle", 16)])
    drawing.append(label(245, 230, "数える上限", "middle", 16))
    save("shortest", 245, "最小の範囲で点数は非負。四つの上限でだけ0になる", drawing)


if __name__ == "__main__":
    inner, outer, mass = (iv.mid().lo / SCALE for iv in enclose_solution())
    frequencies = [float(g)/2 for g in ORDINATES]
    limits = [100*exp(v+outer) for v in (-outer,-inner,inner,outer)]
    weights = [(1-mass)/2, mass/2, mass/2, (1-mass)/2]
    observed()
    contributions(limits, weights, frequencies[0])
    scores(inner, outer, frequencies, limits)
    for frequency in frequencies:
        for phase in (0, pi/2):
            assert abs(sum(w*cos(frequency*log(v/100)+phase) for v,w in zip(limits,weights))) < 1e-12
    observed_mean = sum(w*count_error(v)/v**0.25 for v,w in zip(limits,weights))
    print(f"Normalized observed error after averaging: {observed_mean:+.5f}")
