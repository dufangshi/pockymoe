#!/usr/bin/env python3
"""A credential-free full-screen TUI: SGR mouse selection owns the clipboard.

It never calls a native clipboard helper. Clipboard writes travel only through
stdout, including the direct-plus-tmux-passthrough framing in Claude Code's
installed clipboard routine.
"""
import argparse
import base64
import json
import os
import re
import termios
import tty
import unicodedata

parser = argparse.ArgumentParser()
parser.add_argument('--record', required=True)
parser.add_argument('--framing', choices=['direct', 'tmux-duplex'], default='direct')
args = parser.parse_args()
text = 'MouseClipboard中文'
row, column = 3, 3
mouse = re.compile(rb'\x1b\[<(\d+);(\d+);(\d+)([Mm])')
original = termios.tcgetattr(0)
state = {'ready': True, 'mouseEvents': 0, 'release': False, 'selected': '', 'framing': args.framing}

def record():
    with open(args.record, 'w', encoding='utf-8') as file:
        json.dump(state, file, ensure_ascii=False)

def output(value):
    os.write(1, value.encode('utf-8'))

def selection(start, end):
    low, high = sorted([start, end])
    position, selected = column, ''
    for character in text:
        width = 2 if unicodedata.east_asian_width(character) in ['W', 'F'] else 1
        if position <= high and position + width - 1 >= low:
            selected += character
        position += width
    return selected

try:
    tty.setraw(0)
    output('\x1b[?1049h\x1b[2J\x1b[H\x1b[?25l\x1b[?1000h\x1b[?1002h\x1b[?1006h')
    output('\x1b[1;3HProgram-owned mouse selection; q exits')
    output(f'\x1b[{row};{column}H{text}')
    record()
    buffer, start = b'', None
    while True:
        data = os.read(0, 4096)
        if not data or data in [b'q', b'\x03']:
            break
        buffer += data
        while True:
            match = mouse.search(buffer)
            if not match:
                buffer = buffer[-256:]
                break
            buffer = buffer[match.end():]
            button, x, y = (int(match[index]) for index in [1, 2, 3])
            state['mouseEvents'] += 1
            if match[4] == b'M' and button == 0 and y == row:
                start = x
            elif start is not None and match[4] == b'm' and button & 3 == 0:
                selected = selection(start, x)
                state.update(release=True, selected=selected)
                if selected:
                    encoded = base64.b64encode(selected.encode('utf-8')).decode('ascii')
                    frame = f'\x1b]52;c;{encoded}\x07'
                    if args.framing == 'tmux-duplex':
                        frame += '\x1bPtmux;' + frame.replace('\x1b', '\x1b\x1b') + '\x1b\\'
                    output('\x1b[5;3H已复制' + frame)
                start = None
            record()
finally:
    output('\x1b[?1000l\x1b[?1002l\x1b[?1006l\x1b[?25h\x1b[?1049l')
    termios.tcsetattr(0, termios.TCSADRAIN, original)
