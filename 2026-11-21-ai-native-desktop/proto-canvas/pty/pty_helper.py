"""pty/server.ts から起動する擬似端末の口 (Python の標準ライブラリだけ)。

  python3 -I pty_helper.py COLS ROWS -- CMD...

- stdin のバイト列を端末へ、端末の出力を stdout へそのまま流す
- fd 3 から 1 行ずつ制御命令を読む: "resize COLS ROWS" で TIOCSWINSZ (子には SIGWINCH が届く)
- 子が終わったら、その終了コードで終わる
"""
import fcntl
import os
import pty
import select
import struct
import sys
import termios


def set_size(fd: int, cols: int, rows: int) -> None:
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def main() -> int:
    cols, rows = int(sys.argv[1]), int(sys.argv[2])
    cmd = sys.argv[sys.argv.index("--") + 1 :]
    pid, master = pty.fork()
    if pid == 0:
        os.execvp(cmd[0], cmd)
    set_size(master, cols, rows)
    ctl = 3
    ctl_buf = b""
    fds = [0, master, ctl]
    while True:
        try:
            ready, _, _ = select.select(fds, [], [])
        except InterruptedError:
            continue
        if master in ready:
            try:
                data = os.read(master, 65536)
            except OSError:
                data = b""
            if not data:
                break
            os.write(1, data)
        if 0 in ready:
            data = os.read(0, 65536)
            if not data:
                fds.remove(0)
            else:
                os.write(master, data)
        if ctl in ready:
            data = os.read(ctl, 4096)
            if not data:
                fds.remove(ctl)
            ctl_buf += data
            while b"\n" in ctl_buf:
                line, ctl_buf = ctl_buf.split(b"\n", 1)
                parts = line.decode().split()
                if len(parts) == 3 and parts[0] == "resize":
                    set_size(master, max(10, min(500, int(parts[1]))), max(2, min(200, int(parts[2]))))
    _, status = os.waitpid(pid, 0)
    return os.waitstatus_to_exitcode(status)


if __name__ == "__main__":
    sys.exit(main())
