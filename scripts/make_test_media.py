"""Generate the synthetic test library used by scripts/test_features.py.

Covers the formats a player is usually judged on: HEVC 10-bit + AC3 5.1 +
multiple audio/subtitle tracks + chapters (MKV), XviD/AVI, WMV, VP9/WebM, AV1,
interlaced MPEG-2 TS, FLV, ProRes/MOV, rotated phone video, FLAC with cover art,
MP3, a sidecar .srt, and an episode folder for playlist / natural-sort tests.

usage: python scripts/make_test_media.py [out_dir] [--ffmpeg path]
needs: an ffmpeg build with libx264, libx265, libvpx, libsvtav1, libmp3lame, libopus
"""
import os
import shutil
import subprocess
import sys

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
args = [a for a in sys.argv[1:] if not a.startswith('--')]
out = os.path.abspath(args[0] if args else os.path.join(root, '.cache', 'testmedia'))
ff = os.path.abspath(sys.argv[sys.argv.index('--ffmpeg') + 1]) if '--ffmpeg' in sys.argv else shutil.which('ffmpeg') or 'ffmpeg'
os.makedirs(os.path.join(out, 'series'), exist_ok=True)

SRT = """1
00:00:01,000 --> 00:00:04,000
Hello from the <i>English</i> subtitle track

2
00:00:05,000 --> 00:00:09,000
Second line of English
with two rows

3
00:00:20,000 --> 00:00:30,000
Chapter two subtitle
"""
ASS = r"""[Script Info]
ScriptType: v4.00+
PlayResX: 640
PlayResY: 360

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Arial,28,&H0000FFFF,&H000000FF,&H00000000,&H64000000,-1,0,0,0,100,100,0,0,1,2,1,2,10,10,20,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:06.00,Default,,0,0,0,,{\i1}Hola{\i0} desde ASS amarillo
Dialogue: 0,0:00:07.00,0:00:12.00,Default,,0,0,0,,{\an8}Texto arriba
"""
CHAPTERS = """;FFMETADATA1
title=Chaptered Test
[CHAPTER]
TIMEBASE=1/1000
START=0
END=20000
title=Opening
[CHAPTER]
TIMEBASE=1/1000
START=20000
END=50000
title=The Middle Part
[CHAPTER]
TIMEBASE=1/1000
START=50000
END=90000
title=Finale
"""


def write(name, text):
    with open(os.path.join(out, name), 'w', encoding='utf8', newline='\n') as f:
        f.write(text)


def ffmpeg(*a):
    subprocess.run([ff, '-hide_banner', '-loglevel', 'error', '-y', *a], check=True, cwd=out)
    print('  ok', a[-1])


write('eng.srt', SRT)
write('fre.srt', SRT)
write('sidecar.srt', SRT)
write('spa.ass', ASS)
write('chapters.txt', CHAPTERS)
src = lambda v, d: ['-f', 'lavfi', '-i', f'{v}:d={d}' if 'mandelbrot' not in v else v]  # noqa: E731
tone = lambda f, d, extra='': ['-f', 'lavfi', '-i', f'sine=f={f}:d={d}{extra}']  # noqa: E731

ffmpeg(*src('testsrc2=s=1280x720:r=24', 90), *tone(440, 90, ',aformat=channel_layouts=5.1'), *tone(660, 90),
       '-i', 'eng.srt', '-i', 'spa.ass', '-i', 'chapters.txt',
       '-map', '0:v', '-map', '1:a', '-map', '2:a', '-map', '3', '-map', '4', '-map_metadata', '5', '-map_chapters', '5',
       '-c:v', 'libx265', '-preset', 'ultrafast', '-x265-params', 'log-level=error', '-pix_fmt', 'yuv420p10le',
       '-c:a:0', 'ac3', '-c:a:1', 'aac', '-c:s:0', 'srt', '-c:s:1', 'ass',
       '-metadata:s:a:0', 'language=eng', '-metadata:s:a:0', 'title=Surround', '-metadata:s:a:1', 'language=hin',
       '-metadata:s:s:0', 'language=eng', '-metadata:s:s:1', 'language=spa', 'multi-hevc.mkv')
ffmpeg(*src('testsrc=s=640x480:r=25', 40), *tone(330, 40), '-c:v', 'mpeg4', '-vtag', 'XVID', '-q:v', '4', '-c:a', 'libmp3lame', 'xvid.avi')
ffmpeg(*src('smptebars=s=640x360:r=30', 30), *tone(550, 30), '-c:v', 'wmv2', '-b:v', '1M', '-c:a', 'wmav2', 'old.wmv')
ffmpeg('-f', 'lavfi', '-i', 'mandelbrot=s=854x480:r=30', *tone(500, 30), '-t', '30', '-c:v', 'libvpx-vp9', '-deadline', 'realtime',
       '-cpu-used', '8', '-b:v', '1M', '-c:a', 'libopus', 'vp9.webm')
ffmpeg(*src('testsrc2=s=1280x720:r=30', 20), *tone(700, 20), '-c:v', 'libsvtav1', '-preset', '12', '-c:a', 'flac', 'av1.mkv')
ffmpeg(*src('testsrc=s=720x576:r=25', 30), *tone(300, 30), '-vf', 'interlace', '-c:v', 'mpeg2video', '-flags', '+ilme+ildct',
       '-b:v', '4M', '-c:a', 'mp2', 'broadcast.ts')
ffmpeg(*src('testsrc=s=480x270:r=25', 20), *tone(800, 20), '-c:v', 'flv1', '-c:a', 'libmp3lame', '-ar', '44100', 'old.flv')
ffmpeg(*src('smptehdbars=s=1280x720:r=25', 10), *tone(250, 10), '-c:v', 'prores_ks', '-profile:v', '0', '-c:a', 'pcm_s16le', 'prores.mov')
ffmpeg('-f', 'lavfi', '-i', 'testsrc=s=500x500', '-frames:v', '1', 'cover.jpg')
ffmpeg(*tone(440, 60), '-i', 'cover.jpg', '-map', '0', '-map', '1', '-c:a', 'flac', '-c:v', 'copy', '-disposition:v', 'attached_pic',
       '-metadata', 'title=Test Song', '-metadata', 'artist=VELA Band', '-metadata', 'album=Debug Album', 'song.flac')
ffmpeg(*tone(520, 45), '-c:a', 'libmp3lame', '-b:a', '192k', '-metadata', 'title=Plain MP3', 'plain.mp3')
ffmpeg(*src('testsrc2=s=1280x720:r=30', 40), *tone(600, 40), '-c:v', 'libx264', '-preset', 'ultrafast', '-c:a', 'aac', 'sidecar.mp4')
ffmpeg(*src('testsrc2=s=1280x720:r=30', 15), *tone(600, 15), '-c:v', 'libx264', '-preset', 'ultrafast', '-c:a', 'aac', 'tmp.mp4')
ffmpeg('-display_rotation', '90', '-i', 'tmp.mp4', '-c', 'copy', 'phone.mp4')
for f in ('cover.jpg', 'tmp.mp4'):
    os.remove(os.path.join(out, f))
for e in (1, 2, 10):
    ffmpeg(*src('testsrc2=s=1280x720:r=30', 75), *tone(300 + e * 40, 75), '-c:v', 'libx264', '-preset', 'ultrafast', '-c:a', 'aac',
           os.path.join('series', f'Show S01E{e}.mp4'))
print('test media in', out)
