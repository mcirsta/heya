#!/usr/bin/env python3
"""Native restart/WM-state checks; invoked by ui-offscreen.sh on a private bus/display."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

binary = sys.argv[1]
protocol = sys.argv[2]
runtime = Path(os.environ['XDG_RUNTIME_DIR'])
assert runtime.parent.name.startswith('heya-offscreen.')
assert os.environ.get('DBUS_SESSION_BUS_ADDRESS')
state_file = runtime / 'synthetic-window-state.json'
env = dict(os.environ, HEYA_TEST_WINDOW_STATE=str(state_file), LIBGL_ALWAYS_SOFTWARE='1', SLINT_BACKEND=os.environ.get('HEYA_TEST_RENDERER','winit-skia-software'), QT_LOGGING_RULES='kwin_scripting.debug=true', QT_FORCE_STDERR_LOGGING='1')
env.pop('DISPLAY', None)
env['WAYLAND_DISPLAY']='heya-window-test'
compositor=['kwin_wayland','--virtual','--width','1600','--height','1100','--socket',env['WAYLAND_DISPLAY'],'--no-lockscreen','--no-global-shortcuts','--no-kactivities']
if protocol=='x11':
    # Exercise genuine X11 through a private Xwayland. No standalone X11 WM
    # or connection to the owner's compositor is needed.
    helper=runtime/'capture-display.py'
    helper.write_text('#!/usr/bin/env python3\nimport json,os,pathlib\npathlib.Path(__file__).with_name("display.json").write_text(json.dumps({k:os.environ[k] for k in ("DISPLAY","XAUTHORITY") if k in os.environ}))\n')
    helper.chmod(0o700)
    compositor += ['--xwayland',str(helper)]
compositor_env=dict(env,QT_QPA_PLATFORM='offscreen')
log_path = runtime / 'window-manager.log'
log = log_path.open('w')
wm = subprocess.Popen(compositor, stdout=log, stderr=log, env=compositor_env)
child = None
sequence = 0

def dbus(*args):
    return subprocess.check_output(['qdbus6', 'org.kde.KWin', *args], text=True, env=env, stderr=subprocess.DEVNULL).strip()

def script(pid, action=''):
    global sequence
    sequence += 1
    marker = f'HEYA_WINDOW_{sequence}:'
    path = runtime / 'query-window.js'
    path.write_text('let found=false; for(const w of workspace.windowList()) { if(w.pid === '+str(pid)+' && String(w.caption).startsWith("Heya")) {'+action+'; const r=w.frameGeometry; const b=w.bufferGeometry; console.warn('+json.dumps(marker)+'+JSON.stringify({frame:[r.x,r.y,r.width,r.height],buffer:[b.width,b.height],max:w.maximizeMode,full:w.fullScreen,min:w.minimized})); found=true; }} if(!found) console.warn('+json.dumps(marker)+'+"null");')
    script_id = dbus('/Scripting', 'org.kde.kwin.Scripting.loadScript', str(path), 'heya-window-check')
    try:
        dbus('/Scripting/Script'+script_id, 'org.kde.kwin.Script.run')
        for _ in range(15):
            for line in reversed(log_path.read_text(errors='replace').splitlines()):
                if marker in line:
                    return json.loads(line.split(marker, 1)[1])
            time.sleep(.02)
        return None
    finally:
        dbus('/Scripting', 'org.kde.kwin.Scripting.unloadScript', 'heya-window-check')

def wait_window(predicate=lambda v:True, action=''):
    if action:
        script(child.pid, action)
    deadline = time.monotonic()+12
    result = None
    while time.monotonic() < deadline:
        assert child.poll() is None, 'synthetic app exited early'
        result = script(child.pid)
        if result is not None and predicate(result):
            time.sleep(.15)
            return result
        time.sleep(.08)
    raise AssertionError(f'native window did not reach the requested state: {result}')

def launch(*args):
    global child
    output = (runtime / f'heya-{sequence}.log').open('w')
    child = subprocess.Popen([binary, '--demo', '--demo-chat', '7', '--startup-trace', str(runtime / f'startup-{sequence}.log'), *args], env=env, stdout=output, stderr=output)
    output.close()
    return wait_window()

def close():
    global child
    script(child.pid, 'w.closeWindow()')
    assert child.wait(timeout=6) == 0
    child = None
    assert state_file.exists(), 'window state was not saved on normal close'
    return json.loads(state_file.read_text())

def same_size(a,b):
    return abs(a[2]-b[2])<=2 and abs(a[3]-b[3])<=2

try:
    for _ in range(100):
        if wm.poll() is not None:
            raise AssertionError('private compositor exited: '+log_path.read_text(errors='replace')[-2000:])
        try:
            dbus('/Scripting', 'org.kde.kwin.Scripting.isScriptLoaded', 'heya-window-check')
            break
        except subprocess.CalledProcessError:
            time.sleep(.1)
    if protocol=='x11':
        for _ in range(100):
            display_file=runtime/'display.json'
            if display_file.exists():
                env.update(json.loads(display_file.read_text()))
                env.pop('WAYLAND_DISPLAY',None)
                break
            time.sleep(.1)
        assert env.get('DISPLAY'), 'private Xwayland display was not published'
    launch()
    original = wait_window(lambda v:v['max']==0 and 940<=v['frame'][2]<=980, 'w.setMaximize(false,false); w.frameGeometry={x:130,y:90,width:960,height:740}')
    saved = close()
    print('first-saved', saved, 'frame', original, flush=True)
    assert not saved['maximized'] and not saved['fullscreen']
    assert saved['width']>=520 and saved['height']>=480
    normal = (saved['width'],saved['height'])
    restored = launch()
    restored = wait_window(lambda v:same_size(v['frame'],original['frame']))
    wait_window(lambda v:v['max']==3, 'w.setMaximize(true,true)')
    saved = close()
    assert saved['maximized'] and not saved['fullscreen']
    assert (saved['width'],saved['height'])==normal, ('maximized dimensions replaced normal bounds',saved)
    launch()
    wait_window(lambda v:v['max']==3)
    restored = wait_window(lambda v:v['max']==0 and same_size(v['frame'],original['frame']), 'w.setMaximize(false,false)')
    wait_window(lambda v:v['full'], 'w.fullScreen=true')
    saved = close()
    assert saved['fullscreen'] and (saved['width'],saved['height'])==normal
    launch()
    wait_window(lambda v:v['full'])
    wait_window(lambda v:not v['full'] and same_size(v['frame'],original['frame']) and v['buffer'][0]<=v['frame'][2] and v['buffer'][1]<=v['frame'][3], 'w.fullScreen=false')
    wait_window(lambda v:v['min'], 'w.minimized=true')
    close()
    restored=launch()
    assert not restored['min'] and not restored['full']
    close()
    # Fullscreen must also retain the maximized mode beneath it.
    launch()
    wait_window(lambda v:v['max']==3, 'w.setMaximize(true,true)')
    wait_window(lambda v:v['full'], 'w.fullScreen=true')
    saved=close()
    assert saved['maximized'] and saved['fullscreen']
    assert (saved['width'],saved['height'])==normal
    launch()
    wait_window(lambda v:v['full'])
    wait_window(lambda v:not v['full'] and v['max']==3, 'w.fullScreen=false')
    wait_window(lambda v:v['max']==0 and same_size(v['frame'],original['frame']), 'w.setMaximize(false,false)')
    close()
    # Bad/missing output must not keep a window offscreen or prevent startup.
    saved.update(width=5000,height=3000,maximized=False,fullscreen=False)
    state_file.write_text(json.dumps(saved))
    launch()
    restored=wait_window(lambda v:v['frame'][2]<=1600 and v['frame'][3]<=1100)
    close()
    state_file.write_text('{ corrupt')
    restored=launch()
    close()
    restored=launch('--size','840x620')
    assert 830<=restored['frame'][2]<=880 and 620<=restored['frame'][3]<=680, restored
    close()
    print(f'Window state ({protocol}): normal restart, maximize/restore, fullscreen/restore, minimized close, removed monitor, corrupt state and explicit size passed')
except Exception:
    print('synthetic diagnostics:', log_path.read_text(errors='replace')[-1800:], flush=True)
    for p in sorted(runtime.glob('startup-*.log'),key=lambda p:p.stat().st_mtime,reverse=True)[:3]+sorted(runtime.glob('heya-*.log'),key=lambda p:p.stat().st_mtime,reverse=True)[:3]:
        print(p.name,p.read_text(errors='replace')[-2000:],flush=True)
    if state_file.exists(): print('saved-state',state_file.read_text(),flush=True)
    raise
finally:
    if child is not None and child.poll() is None:
        child.terminate()
        try: child.wait(timeout=5)
        except subprocess.TimeoutExpired: child.kill(); child.wait()
    wm.terminate()
    try: wm.wait(timeout=5)
    except subprocess.TimeoutExpired: wm.kill(); wm.wait()
    log.close()
