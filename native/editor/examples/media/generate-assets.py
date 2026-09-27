"""Generate deterministic local test assets using Python stdlib and FFmpeg."""
from pathlib import Path
import binascii
import struct
import subprocess
import zlib

folder = Path(__file__).resolve().parent
width, height = 320, 160
rows = bytearray()
for y in range(height):
    rows.append(0)
    for x in range(width):
        rgb = ((251, 112, 62), (58, 222, 174), (89, 148, 245), (240, 201, 80))[(x // 80 + y // 80) % 4]
        rows.extend((*rgb, 120 if (x // 40 + y // 40) % 2 else 255))

def chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', binascii.crc32(kind + data))

png = b'\x89PNG\r\n\x1a\n'
png += chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 6, 0, 0, 0))
png += chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b'')
with (folder / 'tiles.png').open('xb') as output:
    output.write(png)
subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-n', '-f', 'lavfi', '-i',
                'testsrc2=size=480x300:rate=24:duration=3', '-c:v', 'libx264',
                '-pix_fmt', 'yuv420p', '-crf', '20', str(folder / 'pattern.mp4')], check=True)
