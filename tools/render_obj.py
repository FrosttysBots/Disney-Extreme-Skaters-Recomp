"""Quick preview renderer for exported OBJ files (pure Python + Pillow).

Painter's algorithm, one color per triangle: vertex color x texture color
sampled at the triangle's centroid UV, times simple directional shading.

usage: python tools/render_obj.py model.obj out.png [yaw_degrees] [pitch_degrees] [zoom] [cull]

Pass `cull` to skip back faces, which lets the camera see into closed rooms.
"""
import math, os, sys
from PIL import Image, ImageDraw

def load_obj(path):
    verts, colors, uvs, faces, mtl_tex = [], [], [], [], {}
    base = os.path.dirname(path); current = None
    for line in open(path):
        parts = line.split()
        if not parts: continue
        if parts[0] == 'v':
            verts.append(tuple(map(float, parts[1:4])))
            colors.append(tuple(map(float, parts[4:7])) if len(parts) >= 7 else (1, 1, 1))
        elif parts[0] == 'vt': uvs.append((float(parts[1]), float(parts[2])))
        elif parts[0] == 'usemtl': current = parts[1]
        elif parts[0] == 'mtllib':
            name = None
            for l in open(os.path.join(base, parts[1])):
                p = l.split()
                if p and p[0] == 'newmtl': name = p[1]
                elif p and p[0] == 'map_Kd': mtl_tex[name] = os.path.join(base, p[1])
        elif parts[0] == 'f':
            idx = [tuple((int(x) - 1) if x else None for x in (c.split('/') + [''])[:2]) for c in parts[1:4]]
            faces.append((idx, current))
    return verts, colors, uvs, faces, mtl_tex

def main():
    path, out = sys.argv[1], sys.argv[2]
    yaw = math.radians(float(sys.argv[3]) if len(sys.argv) > 3 else 35)
    pitch = math.radians(float(sys.argv[4]) if len(sys.argv) > 4 else 35)
    zoom = float(sys.argv[5]) if len(sys.argv) > 5 else 1.0
    cull = len(sys.argv) > 6 and sys.argv[6] == 'cull'
    verts, colors, uvs, faces, mtl_tex = load_obj(path)
    textures = {}
    def tex_color(mtl, uv):
        f = mtl_tex.get(mtl)
        if not f: return (1, 1, 1)
        if f not in textures: textures[f] = Image.open(f).convert('RGB')
        im = textures[f]
        x = int((uv[0] % 1.0) * im.width) % im.width
        y = int((1 - uv[1] % 1.0) * im.height) % im.height  # OBJ v=0 is the bottom
        return tuple(c / 255 for c in im.getpixel((x, y)))

    # Frame the middle 90% of vertices (ignores huge sky/ocean planes).
    def pct(axis, q):
        s = sorted(v[axis] for v in verts); return s[int(q * (len(s) - 1))]
    center = [(pct(a, 0.05) + pct(a, 0.95)) / 2 for a in range(3)]
    radius = max(pct(a, 0.95) - pct(a, 0.05) for a in range(3)) / 2 or 1
    dist = radius * 2.2 / zoom
    # Camera on a sphere around the center, Y up.
    eye = [center[0] + dist * math.cos(pitch) * math.sin(yaw),
           center[1] + dist * math.sin(pitch),
           center[2] + dist * math.cos(pitch) * math.cos(yaw)]
    fwd = [center[i] - eye[i] for i in range(3)]; n = math.sqrt(sum(c * c for c in fwd)); fwd = [c / n for c in fwd]
    right = [fwd[2], 0, -fwd[0]]; n = math.sqrt(sum(c * c for c in right)); right = [-c / n for c in right]
    up = [right[1] * fwd[2] - right[2] * fwd[1], right[2] * fwd[0] - right[0] * fwd[2], right[0] * fwd[1] - right[1] * fwd[0]]
    W, H = 1024, 768; focal = W * 0.9
    proj = []
    for v in verts:
        d = [v[i] - eye[i] for i in range(3)]
        z = sum(d[i] * fwd[i] for i in range(3))
        x = sum(d[i] * right[i] for i in range(3)); y = sum(d[i] * up[i] for i in range(3))
        proj.append((W / 2 + focal * x / z, H / 2 - focal * y / z, z) if z > radius * 0.01 else None)
    light = [0.4, 0.8, 0.3]
    tris = []
    for idx, mtl in faces:
        pts = [proj[i[0]] for i in idx]
        if any(p is None for p in pts): continue
        if cull:
            # Counter-clockwise in world space is clockwise on screen (y points down).
            area = (pts[1][0] - pts[0][0]) * (pts[2][1] - pts[0][1]) - (pts[2][0] - pts[0][0]) * (pts[1][1] - pts[0][1])
            if area > 0: continue
        a, b, c = (verts[i[0]] for i in idx)
        e1 = [b[k] - a[k] for k in range(3)]; e2 = [c[k] - a[k] for k in range(3)]
        nrm = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]]
        ln = math.sqrt(sum(x * x for x in nrm)) or 1
        shade = 0.55 + 0.45 * abs(sum(nrm[k] * light[k] for k in range(3)) / ln)
        vc = [sum(colors[i[0]][k] for i in idx) / 3 for k in range(3)]
        if all(i[1] is not None for i in idx):
            uv = (sum(uvs[i[1]][0] for i in idx) / 3, sum(uvs[i[1]][1] for i in idx) / 3)
            tc = tex_color(mtl, uv)
        else: tc = (1, 1, 1)
        col = tuple(min(255, int(255 * vc[k] * tc[k] * shade)) for k in range(3))
        tris.append((max(p[2] for p in pts), [(p[0], p[1]) for p in pts], col))
    tris.sort(key=lambda t: -t[0])
    img = Image.new('RGB', (W, H), (40, 44, 52)); draw = ImageDraw.Draw(img)
    for _, pts, col in tris: draw.polygon(pts, fill=col)
    img.save(out); print(f"{out}: {len(tris)} triangles drawn")

main()
