#!/usr/bin/env bash
# 画面なしの sway の上で、D-Bus のセッションと AT-SPI のバスを立て、引数のコマンドを実行する。
# GTK の Python バインディングは Ubuntu 24.04 のシステムの Python 3.12 (/usr/bin/python3.12) 用
# 使い方: legacy/run-env.sh <コマンド...>   (sway / at-spi2-core / python3-pyatspi / gtk-3-examples が必要)
set -euo pipefail
RUN=${RUN_DIR:-/tmp/legacy-env}
mkdir -p "$RUN" && chmod 700 "$RUN"
export XDG_RUNTIME_DIR="$RUN"
if [ ! -S "$RUN/wayland-1" ]; then
  printf 'output HEADLESS-1 resolution 1400x800\ndefault_border none\n' > "$RUN/sway.conf"
  WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman sway -c "$RUN/sway.conf" > "$RUN/sway.log" 2>&1 &
  for _ in $(seq 50); do [ -S "$RUN/wayland-1" ] && break; sleep 0.1; done
fi
export WAYLAND_DISPLAY=wayland-1 GDK_BACKEND=wayland NO_AT_BRIDGE=0 GTK_A11Y=atspi
exec dbus-run-session -- bash -c '
  /usr/libexec/at-spi-bus-launcher --launch-immediately &
  sleep 1
  "$@"' _ "$@"
