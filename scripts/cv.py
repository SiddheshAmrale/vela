"""Tiny client for VELA's automation channel (named pipe on Windows,
Unix socket elsewhere).

usage: python scripts/cv.py <verb> [lines...]
  verbs: open <path..> | enqueue <path..> | state | get <prop> | mpv <args..>
         action "<name> <arg>" | shot <file.bmp> | quit
"""
import os
import socket
import sys
import tempfile
import time

NUL = b'\x00'


def user_tag():
    user = os.environ.get('USERNAME') or os.environ.get('USER') or 'user'
    return ''.join(c for c in user if c.isalnum())


def endpoint():
    if os.name == 'nt':
        return r'\\.\pipe\Vela-' + user_tag()
    base = os.environ.get('XDG_RUNTIME_DIR') or tempfile.gettempdir()
    return os.path.join(base, f'vela-{user_tag()}.sock')


def _exchange(read, write, msg):
    write(msg)
    out = b''
    while not out.endswith(NUL):
        chunk = read(65536)
        if not chunk:
            break
        out += chunk
    return out.rstrip(NUL).decode('utf-8', 'replace')


def send(verb, *lines, timeout=10.0):
    msg = '\n'.join((verb,) + lines).encode('utf-8') + NUL
    deadline = time.time() + timeout
    while True:
        try:
            if os.name == 'nt':
                with open(endpoint(), 'r+b', buffering=0) as f:
                    return _exchange(f.read, f.write, msg)
            with socket.socket(socket.AF_UNIX) as sock:
                sock.connect(endpoint())
                return _exchange(sock.recv, sock.sendall, msg)
        except OSError:
            if time.time() > deadline:
                raise
            time.sleep(0.05)


if __name__ == '__main__':
    print(send(sys.argv[1], *sys.argv[2:]))
