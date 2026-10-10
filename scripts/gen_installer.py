"""Generates installer/vela.nsi from installer/vela.nsi.in, expanding the
file-type registration for every extension VELA understands (read from
src/platform.rs so the installer and the player never disagree)."""
import os
import re

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
src = open(os.path.join(root, 'src', 'platform.rs'), encoding='utf8').read()


def grab(name):
    m = re.search(name + r': &\[&str\] = &\[(.*?)\];', src, re.S)
    return re.findall(r'"([a-z0-9]+)"', m.group(1))


video = grab('VIDEO_EXT')
audio = grab('AUDIO_EXT')
playlists = [e for e in grab('PLAYLIST_EXT') if e != 'cue']

assoc = '\n'.join(
    [f'  !insertmacro AssocVideo "{e}"' for e in video + playlists] + [f'  !insertmacro AssocAudio "{e}"' for e in audio]
)
unassoc = '\n'.join(f'  !insertmacro UnAssoc "{e}"' for e in video + audio + playlists)

tpl = open(os.path.join(root, 'installer', 'vela.nsi.in'), encoding='utf8').read()
out = tpl.replace(';@ASSOC@', assoc).replace(';@UNASSOC@', unassoc)
with open(os.path.join(root, 'installer', 'vela.nsi'), 'w', encoding='utf-8-sig') as f:
    f.write(out)
print(f'installer/vela.nsi: {len(video)} video, {len(audio)} audio, {len(playlists)} playlist types')
