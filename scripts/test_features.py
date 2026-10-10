"""End-to-end feature test for VELA.

Drives the real app through its automation pipe (scripts/cv.py), checks the
player state reported by mpv after every action and saves screenshots of each
UI state.

usage: python scripts/test_features.py <path-to-vela-binary> <testmedia-dir> <out-dir>
"""
import json
import os
import subprocess
import sys
import threading
import time
import http.server
import functools

sys.path.insert(0, os.path.dirname(__file__))
from cv import send  # noqa: E402

EXE, MEDIA, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(OUT, exist_ok=True)
results = []


def st():
    return json.loads(send('state'))


def act(a):
    r = send('action', a)
    assert r == 'ok', f'{a} -> {r}'


def wait(pred, timeout=8.0, step=0.05):
    end = time.time() + timeout
    last = None
    while time.time() < end:
        try:
            last = st()
            if pred(last):
                return last
        except Exception:
            pass
        time.sleep(step)
    return last


def check(name, cond, detail=''):
    results.append((name, bool(cond), detail))
    print(('PASS ' if cond else 'FAIL ') + name + (f'  [{detail}]' if detail and not cond else ''))


def shot(name):
    path = os.path.join(OUT, name + '.bmp')
    send('shot', path)
    try:
        from PIL import Image
        Image.open(path).save(os.path.join(OUT, name + '.png'))
        os.remove(path)
    except Exception:
        pass


def media(*parts):
    return os.path.join(MEDIA, *parts)


def open_file(p, timeout=10):
    t0 = time.time()
    send('open', p)
    s = wait(lambda s: not s['idle'] and s['path'].lower() == p.lower() and (s['time'] or 0) > 0.2, timeout)
    return s, time.time() - t0


# ---------------------------------------------------------------------------
proc = subprocess.Popen([EXE])
time.sleep(1.0)
s = wait(lambda s: s['idle'] is True, 10)
check('startup: app responds, idle home', s and s['idle'])
send('mpv', 'set', 'pause', 'no')
shot('01-home')
check('startup: memory at idle (MB)', s and s['memory_mb'] < 400, f"{s and s['memory_mb']:.0f} MB")
print(f"   idle memory: {s['memory_mb']:.0f} MB")

# ---- Format matrix ---------------------------------------------------------
formats = [
    ('multi-hevc.mkv', 'HEVC 10-bit + AC3 5.1 + AAC + SRT + ASS (MKV)', True),
    ('xvid.avi', 'XviD + MP3 (AVI)', True),
    ('old.wmv', 'WMV2 + WMA (ASF)', True),
    ('vp9.webm', 'VP9 + Opus (WebM)', True),
    ('av1.mkv', 'AV1 + FLAC (MKV)', True),
    ('broadcast.ts', 'MPEG-2 interlaced + MP2 (TS)', True),
    ('old.flv', 'Sorenson + MP3 (FLV)', True),
    ('prores.mov', 'ProRes + PCM (MOV)', True),
    ('phone.mp4', 'H.264 rotated 90deg (MP4)', True),
    ('sidecar.mp4', 'H.264 + AAC with sidecar .srt', True),
    ('song.flac', 'FLAC with cover art', False),
    ('plain.mp3', 'MP3 audio', False),
]
for fname, label, video in formats:
    s, dt = open_file(media(fname))
    ok = s and not s['idle'] and s['error'] is None and (s['time'] or 0) > 0.2 and s['aid'] is not None
    if video:
        ok = ok and s['vid'] is not None
    check(f'plays {label}', ok, json.dumps({k: s.get(k) for k in ('time', 'vid', 'aid', 'error', 'hwdec')}) if s else 'no state')
    print(f'   time to playing: {dt*1000:.0f} ms, hwdec={s.get("hwdec") if s else None}')
    shot('fmt-' + fname.replace('.', '_'))

# ---- Core transport --------------------------------------------------------
open_file(media('multi-hevc.mkv'))
send('mpv', 'set', 'pause', 'no')
act('toggle-pause')
s = wait(lambda s: s['pause'])
check('pause', s['pause'])
act('toggle-pause')
s = wait(lambda s: not s['pause'])
check('resume', not s['pause'])
act('seek 30')
s = wait(lambda s: abs((s['time'] or 0) - 30) < 1.0)
check('seek absolute to 30s', abs((s['time'] or 0) - 30) < 1.0, s['time'])
act('seek-by 10')
s = wait(lambda s: abs((s['time'] or 0) - 40) < 1.5)
check('seek +10s', abs((s['time'] or 0) - 40) < 1.5, s['time'])
send('action', 'key left')
s = wait(lambda s: abs((s['time'] or 0) - 35) < 1.5)
check('key Left = -5s', abs((s['time'] or 0) - 35) < 1.5, s['time'])
send('action', 'key 5')
s = wait(lambda s: abs((s['time'] or 0) - 45) < 1.5)
check('key 5 = jump to 50%', abs((s['time'] or 0) - 45) < 1.5, s['time'])
send('action', 'key space')
s = wait(lambda s: s['pause'])
check('key Space pauses', s['pause'])
t0 = s['time']
send('action', 'key .')
s = wait(lambda s: s['time'] and s['time'] > t0 + 0.01, 3)
check('frame step forward', s['time'] > t0, f"{t0} -> {s['time']}")
send('action', 'key space')
wait(lambda s: not s['pause'])

# ---- Volume / speed ---------------------------------------------------------
act('volume 50')
s = wait(lambda s: abs(s['volume'] - 50) < 0.1)
check('set volume 50%', abs(s['volume'] - 50) < 0.1, s['volume'])
send('action', 'key up')
s = wait(lambda s: abs(s['volume'] - 55) < 0.1)
check('key Up = +5%', abs(s['volume'] - 55) < 0.1, s['volume'])
act('volume 150')
s = wait(lambda s: abs(s['volume'] - 150) < 0.1)
check('volume boost 150%', abs(s['volume'] - 150) < 0.1, s['volume'])
act('mute')
s = wait(lambda s: s['mute'])
check('mute', s['mute'])
act('mute')
s = wait(lambda s: not s['mute'])
check('unmute', not s['mute'])
act('volume 100')
act('speed 1.5')
s = wait(lambda s: abs(s['speed'] - 1.5) < 0.01)
check('speed 1.5x', abs(s['speed'] - 1.5) < 0.01, s['speed'])
send('action', 'key ]')
s = wait(lambda s: abs(s['speed'] - 1.75) < 0.01)
check('key ] = faster (1.75x)', abs(s['speed'] - 1.75) < 0.01, s['speed'])
send('action', 'key backspace')
s = wait(lambda s: abs(s['speed'] - 1.0) < 0.01)
check('Backspace = normal speed', abs(s['speed'] - 1.0) < 0.01, s['speed'])

# ---- Subtitles --------------------------------------------------------------
act('seek 2')
act('sub 1')
s = wait(lambda s: s['sid'] == 1)
check('select embedded SRT subtitle', s['sid'] == 1, s['sid'])
time.sleep(0.6)
act('activity')
shot('sub-srt')
act('sub 2')
s = wait(lambda s: s['sid'] == 2)
check('select embedded ASS subtitle', s['sid'] == 2, s['sid'])
act('seek 1.5')
time.sleep(0.6)
shot('sub-ass')
act('sub-delay 0.5')
s = wait(lambda s: abs(s['sub_delay'] - 0.5) < 0.01)
check('subtitle delay +0.5s', abs(s['sub_delay'] - 0.5) < 0.01, s['sub_delay'])
send('action', 'key g')
s = wait(lambda s: abs(s['sub_delay'] - 0.4) < 0.01)
check('key G = subtitle delay -0.1s', abs(s['sub_delay'] - 0.4) < 0.01, s['sub_delay'])
send('action', 'key c')
s = wait(lambda s: not s['sub_visible'])
check('key C hides subtitles', not s['sub_visible'])
send('action', 'key c')
s = wait(lambda s: s['sub_visible'])
check('key C shows subtitles', s['sub_visible'])
act('sub-add ' + media('fre.srt'))
s = wait(lambda s: any(t['external'] and t['type'] == 'sub' and t['selected'] for t in s['tracks']))
check('load external subtitle file', any(t['external'] and t['type'] == 'sub' and t['selected'] for t in s['tracks']))
act('sub-scale 1.6')
act('sub-back 0.6')
time.sleep(0.3)
shot('sub-styled')
act('sub-scale 1.0')
act('sub-back 0')
act('sub no')
s = wait(lambda s: s['sid'] is None)
check('subtitles off', s['sid'] is None)

# ---- Audio tracks -----------------------------------------------------------
act('audio 2')
s = wait(lambda s: s['aid'] == 2)
check('switch audio track (Hindi AAC)', s['aid'] == 2, s['aid'])
send('action', 'key b')
s = wait(lambda s: s['aid'] == 1)
check('key B cycles audio track', s['aid'] == 1, s['aid'])
act('audio-delay 0.25')
s = wait(lambda s: abs(s['audio_delay'] - 0.25) < 0.01)
check('audio delay', abs(s['audio_delay'] - 0.25) < 0.01, s['audio_delay'])
act('audio-delay 0')

# ---- Chapters ---------------------------------------------------------------
act('seek 5')
time.sleep(0.3)
act('chapter 1')
s = wait(lambda s: s['chapter'] == 1)
check('next chapter', s['chapter'] == 1 and abs(s['time'] - 20) < 1.5, (s['chapter'], s['time']))

# ---- Video ------------------------------------------------------------------
for a, key, expect in [('aspect 4:3', 'aspect', lambda v: abs(v - 4 / 3) < 0.01), ('aspect fill', 'panscan', lambda v: v > 0.5), ('aspect stretch', 'keepaspect', lambda v: v is False), ('aspect auto', 'aspect', lambda v: v <= 0)]:
    act(a)
    s = wait(lambda s: expect(s[key]))
    check(f'video {a}', expect(s[key]), s[key])
    if a == 'aspect 4:3':
        time.sleep(0.2)
        shot('aspect-4x3')
act('zoom 0.5')
s = wait(lambda s: abs(s['zoom'] - 0.5) < 0.01)
check('video zoom', abs(s['zoom'] - 0.5) < 0.01)
act('rotate 90')
s = wait(lambda s: s['rotate'] == 90)
check('video rotate 90', s['rotate'] == 90)
time.sleep(0.2)
shot('rotate-90')
act('brightness 30')
s = wait(lambda s: s['brightness'] == 30)
check('brightness', s['brightness'] == 30)
act('reset-video')
s = wait(lambda s: s['rotate'] == 0 and s['brightness'] == 0 and abs(s['zoom']) < 0.01)
check('reset picture', s['rotate'] == 0 and s['brightness'] == 0)

# ---- Window -----------------------------------------------------------------
act('fullscreen')
s = wait(lambda s: s['fullscreen'])
check('fullscreen on', s['fullscreen'])
time.sleep(0.5)
act('activity')
shot('fullscreen')
send('action', 'key esc')
s = wait(lambda s: not s['fullscreen'])
check('Esc leaves fullscreen', not s['fullscreen'])
act('ontop')
s = wait(lambda s: s['on_top'])
check('always on top', s['on_top'])
act('ontop')

# ---- Audio effects ----------------------------------------------------------
act('eq bass')
s = wait(lambda s: 'equalizer' in (s['af'] or ''))
check('equalizer preset applies audio filter', 'equalizer' in (s['af'] or ''), s['af'])
act('normalize yes')
s = wait(lambda s: 'dynaudnorm' in (s['af'] or ''))
check('loudness normalization', 'dynaudnorm' in (s['af'] or ''), s['af'])
act('normalize no')
act('eq flat')
s = wait(lambda s: (s['af'] or '') == '')
check('equalizer flat clears filters', (s['af'] or '') == '', s['af'])

# ---- Repeat / A-B -----------------------------------------------------------
act('loop-file yes')
s = wait(lambda s: s['loop_file'])
check('repeat this file', s['loop_file'])
act('loop-file no')
act('seek 10')
time.sleep(0.2)
act('ab')
time.sleep(0.6)
act('ab')
s = wait(lambda s: s['ab'][0] is not None and s['ab'][1] is not None)
check('A-B repeat points set', s['ab'][0] is not None and s['ab'][1] is not None, s['ab'])
time.sleep(1.2)
s = st()
check('A-B repeat loops inside range', s['ab'][0] - 0.3 <= s['time'] <= s['ab'][1] + 0.3, (s['ab'], s['time']))
act('ab')
s = wait(lambda s: s['ab'][0] is None)
check('A-B repeat cleared', s['ab'][0] is None)

# ---- Deinterlace -------------------------------------------------------------
open_file(media('broadcast.ts'))
act('deinterlace yes')
s = wait(lambda s: s['deinterlace'] in ('yes', 'true'))
check('deinterlace on (interlaced MPEG-2)', s['deinterlace'] in ('yes', 'true'), s['deinterlace'])
act('deinterlace no')

# ---- Hover thumbnail ----------------------------------------------------------
open_file(media('multi-hevc.mkv'))
act('hover 42')
s = wait(lambda s: s['hover_thumb'] is not None, 6)
check('seek-bar preview thumbnail generated', s['hover_thumb'] is not None, s['hover_thumb'])

# ---- Screenshot --------------------------------------------------------------
pics = os.path.join(os.path.expanduser('~'), 'Pictures', 'VELA')
before = set(os.listdir(pics)) if os.path.isdir(pics) else set()
act('screenshot')
time.sleep(1.5)
after = set(os.listdir(pics)) if os.path.isdir(pics) else set()
check('screenshot saved to Pictures/VELA', len(after - before) >= 1, list(after - before))

# ---- Menus (visual) -----------------------------------------------------------
act('seek 3')
for m in ['settings', 'subs', 'audio', 'speed', 'playlist', 'context']:
    act('menu ' + m)
    time.sleep(0.35)
    s = st()
    check(f'menu opens: {m}', s['menu'].lower() == m, s['menu'])
    shot('menu-' + m)
for p in ['speed', 'subs', 'substyle', 'audio', 'video', 'eq', 'repeat', 'sleep', 'prefs']:
    act('page ' + p)
    time.sleep(0.3)
    shot('page-' + p)
act('menu none')
send('action', 'key ?')
time.sleep(0.3)
s = st()
check('shortcut help (?)', s['help'])
shot('help')
send('action', 'key esc')
send('action', 'key i')
time.sleep(0.8)
s = st()
check('stats overlay (I)', s['info'])
shot('stats')
send('action', 'key i')
act('menu url')
time.sleep(0.3)
shot('url-dialog')
act('menu none')

# ---- Sleep timer ---------------------------------------------------------------
act('sleep 30')
s = st()
check('sleep timer set', s['sleep'] is not None and s['sleep'] > 1700, s['sleep'])
act('sleep 0')

# ---- Playlist: natural sort, next/prev, up next --------------------------------
s, _ = open_file(media('series', 'Show S01E2.mp4'))
names = [os.path.basename(p) for p in s['playlist']]
check('folder auto-playlist in natural order (E1, E2, E10)', names == ['Show S01E1.mp4', 'Show S01E2.mp4', 'Show S01E10.mp4'], names)
check('started at the opened file', s['playlist_pos'] == 1, s['playlist_pos'])
act('next')
s = wait(lambda s: s['playlist_pos'] == 2 and (s['time'] or 0) > 0.1)
check('next file', s['playlist_pos'] == 2)
act('prev')
s = wait(lambda s: s['playlist_pos'] == 1 and (s['time'] or 0) > 0.1)
check('previous file', s['playlist_pos'] == 1)
act('seek 68')
time.sleep(0.8)
act('activity')
shot('up-next')
s = wait(lambda s: s['playlist_pos'] == 2, 12)
check('autoplay advances to next file at end', s['playlist_pos'] == 2, s['playlist_pos'])

# ---- Resume + continue watching ---------------------------------------------------
open_file(media('series', 'Show S01E1.mp4'))
act('seek 40')
time.sleep(0.5)
act('stop')
s = wait(lambda s: s['idle'])
check('back to home', s['idle'])
time.sleep(2.5)  # posters render in background
shot('home-continue')
s = st()
check('continue-watching poster thumbnails', s['posters'] >= 1, s['posters'])
s, _ = open_file(media('series', 'Show S01E1.mp4'))
check('resumes where you left off', abs((s['time'] or 0) - 40) < 2.0, s['time'])

# ---- Network stream (local HTTP server) --------------------------------------------
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=MEDIA)
handler.log_message = lambda *a: None
httpd = http.server.ThreadingHTTPServer(('127.0.0.1', 8765), handler)
threading.Thread(target=httpd.serve_forever, daemon=True).start()
act('url http://127.0.0.1:8765/vp9.webm')
s = wait(lambda s: not s['idle'] and s['path'].startswith('http') and (s['time'] or 0) > 0.3, 10)
check('plays network URL (HTTP)', s and (s['time'] or 0) > 0.3, s and s['path'])
httpd.shutdown()

# ---- Single instance: second launch hands file over -----------------------------------
p2 = subprocess.Popen([EXE, media('xvid.avi')])
p2.wait(timeout=10)
s = wait(lambda s: s['path'].lower().endswith('xvid.avi'), 8)
check('2nd launch forwards file to running player', s['path'].lower().endswith('xvid.avi') and proc.poll() is None, s['path'])

# Explorer multi-select launches one process per file, in random order.
ps = [subprocess.Popen([EXE, media('series', n)]) for n in ['Show S01E10.mp4', 'Show S01E1.mp4', 'Show S01E2.mp4']]
for p in ps:
    p.wait(timeout=15)
s = wait(lambda s: len(s['playlist']) == 3 and 'series' in s['path'], 8)
names = [os.path.basename(p) for p in s['playlist']]
check('multi-select "Open with" -> one sorted playlist', names == ['Show S01E1.mp4', 'Show S01E2.mp4', 'Show S01E10.mp4'], names)

# ---- Performance snapshot ---------------------------------------------------------
open_file(media('multi-hevc.mkv'))
send('mpv', 'set', 'pause', 'no')
time.sleep(3)
s = st()
print(f"   memory while playing HEVC: {s['memory_mb']:.0f} MB, hwdec={s['hwdec']}")
check('memory while playing (MB)', s['memory_mb'] < 450, f"{s['memory_mb']:.0f}")

send('quit')
try:
    proc.wait(timeout=10)
    check('clean exit', proc.returncode == 0, proc.returncode)
except subprocess.TimeoutExpired:
    proc.kill()
    check('clean exit', False, 'timeout')

passed = sum(1 for r in results if r[1])
print(f'\n{passed}/{len(results)} checks passed')
with open(os.path.join(OUT, 'results.json'), 'w') as f:
    json.dump(results, f, indent=1)
