"""Per-pixel textured preview of an exported OBJ, aimed at one texture.

Z-buffered, perspective-correct UVs, nearest sampling. Pure Python, so keep
the output small. The camera is placed in front of the surfaces that use the
given texture, looking along their average normal.

usage: python tools/render_textured.py model.obj out.png <texture checksum hex> [distance scale]
"""
import math, os, sys
from PIL import Image

def load(path):
    V, C, T, F, mtl_tex = [], [], [], [], {}
    base = os.path.dirname(path); cur = None
    for line in open(path):
        p = line.split()
        if not p: continue
        if p[0] == 'v':
            V.append(tuple(map(float, p[1:4]))); C.append(tuple(map(float, p[4:7])) if len(p) >= 7 else (1, 1, 1))
        elif p[0] == 'vt': T.append((float(p[1]), float(p[2])))
        elif p[0] == 'usemtl': cur = p[1]
        elif p[0] == 'mtllib':
            name = None
            for l in open(os.path.join(base, p[1])):
                q = l.split()
                if q and q[0] == 'newmtl': name = q[1]
                elif q and q[0] == 'map_Kd': mtl_tex[name] = os.path.join(base, q[1])
        elif p[0] == 'f':
            F.append(([tuple((int(x) - 1) if x else None for x in (c.split('/') + [''])[:2]) for c in p[1:4]], cur))
    return V, C, T, F, mtl_tex

def sub(a, b): return [a[i] - b[i] for i in range(3)]
def cross(a, b): return [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]]
def norm(a):
    l = math.sqrt(sum(x * x for x in a)) or 1; return [x / l for x in a]

def main():
    path, out, focus = sys.argv[1], sys.argv[2], sys.argv[3].lower()
    scale = float(sys.argv[4]) if len(sys.argv) > 4 else 1.6
    V, C, T, F, mtl_tex = load(path)
    targets = [f for f in F if os.path.basename(mtl_tex.get(f[1], '')).startswith(focus)]
    if not targets: sys.exit(f"no faces use texture {focus}")
    pts = [V[i[0]] for f in targets for i in f[0]]
    center = [sum(p[k] for p in pts) / len(pts) for k in range(3)]
    size = max(max(p[k] for p in pts) - min(p[k] for p in pts) for k in range(3))
    n = [0, 0, 0]
    for idx, _ in targets:
        a, b, c = (V[i[0]] for i in idx); g = cross(sub(b, a), sub(c, a)); n = [n[k] + g[k] for k in range(3)]
    n = norm(n)
    eye = [center[k] + n[k] * size * scale for k in range(3)]
    fwd = norm(sub(center, eye))
    world_up = [0, 1, 0] if abs(fwd[1]) < 0.9 else [0, 0, 1]
    right = norm(cross(fwd, world_up)); up = cross(right, fwd)
    W = H = 512; focal = W * 0.8
    def project(v):
        d = sub(v, eye); z = sum(d[i] * fwd[i] for i in range(3))
        if z <= size * 0.02: return None
        return (W / 2 + focal * sum(d[i] * right[i] for i in range(3)) / z,
                H / 2 - focal * sum(d[i] * up[i] for i in range(3)) / z, z)
    P = [project(v) for v in V]
    img = Image.new('RGB', (W, H), (40, 44, 52)); px = img.load()
    zbuf = [[float('inf')] * W for _ in range(H)]
    cache = {}
    for idx, mtl in F:
        p = [P[i[0]] for i in idx]
        if any(q is None for q in p): continue
        tex = mtl_tex.get(mtl)
        if tex and tex not in cache: cache[tex] = Image.open(tex).convert('RGB')
        im = cache.get(tex) if tex else None
        uv = [T[i[1]] if i[1] is not None else (0, 0) for i in idx]
        col = [C[i[0]] for i in idx]
        (x0, y0, z0), (x1, y1, z1), (x2, y2, z2) = p
        area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0)
        if abs(area) < 1e-9: continue
        for y in range(max(0, int(min(y0, y1, y2))), min(H, int(max(y0, y1, y2)) + 1)):
            for x in range(max(0, int(min(x0, x1, x2))), min(W, int(max(x0, x1, x2)) + 1)):
                cx, cy = x + 0.5, y + 0.5
                w0 = ((x1 - cx) * (y2 - cy) - (x2 - cx) * (y1 - cy)) / area
                w1 = ((x2 - cx) * (y0 - cy) - (x0 - cx) * (y2 - cy)) / area
                w2 = 1 - w0 - w1
                if w0 < 0 or w1 < 0 or w2 < 0: continue
                iz = w0 / z0 + w1 / z1 + w2 / z2; z = 1 / iz
                if z >= zbuf[y][x]: continue
                zbuf[y][x] = z
                u = (w0 * uv[0][0] / z0 + w1 * uv[1][0] / z1 + w2 * uv[2][0] / z2) * z
                v = (w0 * uv[0][1] / z0 + w1 * uv[1][1] / z1 + w2 * uv[2][1] / z2) * z
                vc = [min(1, w0 * col[0][k] + w1 * col[1][k] + w2 * col[2][k]) for k in range(3)]
                if im:
                    tc = im.getpixel((int((u % 1) * im.width) % im.width, int((1 - v % 1) * im.height) % im.height))
                else: tc = (200, 200, 200)
                px[x, y] = tuple(int(tc[k] * (0.35 + 0.65 * vc[k])) for k in range(3))
    img.save(out); print(out, "focus faces:", len(targets))

main()
