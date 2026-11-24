"""Rebuild the opaque comparison figure with uv run --script draw.py."""
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["matplotlib>=3.8,<4"]
# ///
from math import acos, cos, pi
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
from matplotlib import font_manager

# Prefer Japanese system fonts on macOS and Linux.
for name in ("Hiragino Sans", "Noto Sans CJK JP", "Yu Gothic"):
    if any(font.name == name for font in font_manager.fontManager.ttflist):
        plt.rcParams["font.family"] = [name, "DejaVu Sans"]
        break
else:
    raise RuntimeError("Install a Japanese font such as Noto Sans CJK JP.")

ink = "#173042"
teal = "#006b72"
rust = "#a13f1d"
plt.rcParams.update({"text.color": ink, "axes.unicode_minus": False, "font.size": 13})
fig, ax = plt.subplots(figsize=(8.4, 4.9))
fig.subplots_adjust(left=.08, right=.92, bottom=.09, top=.97)
ax.set(xlim=(-.39, .39), ylim=(-.54, 2.8))
ax.axis("off")
ax.set_facecolor("white")

inner = acos(.75)/(2*pi)
rows = [
    (1.91, "時刻を自由に選ぶ", [-1/3, -inner, inner, 1/3],
     [7/30, 4/15, 4/15, 7/30], ["7/30", "4/15", "4/15", "7/30"], 1/6, teal),
    (.41, "等間隔の五点を使う", [-1/3, -1/6, 0, 1/6, 1/3],
     [1/5]*5, ["1/5"]*5, 1/5, rust),
]
for y, title, times, weights, labels, target, color in rows:
    moments = [sum(w*cos(2*pi*k*t) for w, t in zip(weights, times)) for k in (1, 2, 3)]
    assert np.allclose(moments, [target, -target, target])
    ax.text(-1/3, y+.54, title, size=17, weight="bold", color=color)
    ax.text(1/3, y+.54, "最適な残留振幅  " + ("1/6" if target == 1/6 else "1/5"),
            ha="right", size=15)
    ax.plot([-1/3, 1/3], [y, y], color="#596e78", linewidth=1.5)
    ax.scatter(times, [y]*len(times), s=105, color=color, zorder=3)
    for t, label in zip(times, labels):
        ax.text(t, y+.12, label, ha="center", va="bottom", size=12)
    for t in [-1/3, 0, 1/3]:
        ax.text(t, y-.17, {-1/3:"−1/3", 0:"0", 1/3:"+1/3"}[t], ha="center", va="top", size=12)
ax.text(0, -.43, "中心からの時間（基本周期を 1 とする）", ha="center")
fig.savefig(Path(__file__).with_name("clock.png"), dpi=170, facecolor="white", transparent=False)

fig, ax = plt.subplots(figsize=(8.4, 3.7))
fig.subplots_adjust(left=.07, right=.94, top=.96, bottom=.08)
ax.set(xlim=(.48, 1.02), ylim=(-.37, 2.59))
ax.axis("off")
for y, title, ticks in [
    (1.86, "理想位置の両隣", [.65, .90, 1.0]),
    (.95, "外側に 0.55 を追加", [.55, .65, .90, 1.0]),
    (.04, "内側に 0.75 を追加", [.55, .65, .75, .90, 1.0]),
]:
    a = max(x for x in ticks if x <= .75)
    b = min(x for x in ticks if x >= .75)
    loss = 0 if a == b else 8*(.75-a)*(b-.75)/(3*(3+4*a*b))
    ax.text(.50, y+.42, title, color=ink)
    ax.text(1.0, y+.42, f"残留振幅  {1/6+loss:.4f}", ha="right", color=ink)
    ax.plot([.50, 1], [y, y], color="#74828a", lw=1)
    if a != b:
        ax.plot([a, b], [y, y], color=teal, lw=4)
        ax.plot(.75, y, marker="|", ms=15, color=rust, mew=2)
    for x in ticks:
        ax.scatter(x, y, color=teal if x in (a,b) else "#657580", s=65, zorder=3)
        ax.text(x, y-.10, f"{x:.2f}", ha="center", va="top", fontsize=11)
ax.text(.75, 2.45, "理想位置  0.75", ha="center", color=rust)
fig.savefig(Path(__file__).with_name("bracket.png"), dpi=170, facecolor="white", transparent=False)
