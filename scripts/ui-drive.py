"""Drive the app under Xvfb with real pointer/keyboard events (XTEST) and take screenshots.
usage: drive.py <display> <step>...   steps: move:x,y  click:x,y  rclick:x,y  key:<keysym>  type:<text>  sleep:<s>  shot:<file>  focus
"""
import subprocess, sys, time
from Xlib import X, XK, display
from Xlib.ext import xtest
d = display.Display(sys.argv[1]); root = d.screen().root
def win():
    kids = root.query_tree().children
    best = max(kids, key=lambda w: (w.get_geometry().width * w.get_geometry().height) if w.get_attributes().map_state == X.IsViewable else 0)
    return best
def move(x, y): xtest.fake_input(d, X.MotionNotify, x=x, y=y); d.sync(); time.sleep(0.12)
def button(b, x, y):
    move(x - 3, y - 2); move(x, y)
    xtest.fake_input(d, X.ButtonPress, b); d.sync(); time.sleep(0.08)
    xtest.fake_input(d, X.ButtonRelease, b); d.sync(); time.sleep(0.35)
def key(sym):
    mods = sym.split('+'); k = mods.pop()
    codes = [d.keysym_to_keycode(XK.string_to_keysym(m)) for m in mods] + [d.keysym_to_keycode(XK.string_to_keysym(k))]
    for c in codes: xtest.fake_input(d, X.KeyPress, c); d.sync(); time.sleep(0.03)
    for c in reversed(codes): xtest.fake_input(d, X.KeyRelease, c); d.sync(); time.sleep(0.03)
    time.sleep(0.25)
for step in sys.argv[2:]:
    op, _, arg = step.partition(':')
    if op == 'focus': w = win(); w.set_input_focus(X.RevertToParent, X.CurrentTime); d.sync(); g = w.get_geometry(); print('window', g.width, g.height)
    elif op == 'move': move(*map(int, arg.split(',')))
    elif op == 'click': button(1, *map(int, arg.split(',')))
    elif op == 'rclick': button(3, *map(int, arg.split(',')))
    elif op == 'key': key(arg)
    elif op == 'type':
        for ch in arg: key({' ': 'space', '-': 'minus', '.': 'period', '/': 'slash'}.get(ch, ch))
    elif op == 'sleep': time.sleep(float(arg))
    elif op == 'shot': subprocess.run(['import', '-display', sys.argv[1], '-window', 'root', arg], check=True)
