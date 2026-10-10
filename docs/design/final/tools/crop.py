import sys
from PIL import Image
src, prefix, step = sys.argv[1], sys.argv[2], int(sys.argv[3]) if len(sys.argv)>3 else 1000
im = Image.open(src); w, h = im.size
i = 0
for y in range(0, h, step):
    im.crop((0, y, w, min(h, y+step))).save(f"{prefix}{i}.png"); i += 1
print(w, h, i)
