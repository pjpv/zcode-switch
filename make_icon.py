import zlib, struct, math, io, os

# Z·SWITCH 应用图标生成：
# 读取 logo-src.png（品牌 Z 字箭头 logo，1024 RGBA），
# 应用 iOS 风格圆角裁切（1024 坐标，半径 232，超采样抗锯齿），
# 输出 icon-src-1024.png 供 `npx tauri icon` 生成全套平台图标。

SRC = "logo-src.png"
OUT_NAME = "icon-src-1024.png"
OUT = 1024
CORNER = 232.0
SS = 4                     # 圆角抗锯齿超采样
AA = CORNER / SS           # 覆盖率过渡宽度（输出像素）

# ---------- PNG 解码（纯标准库） ----------

def read_png(path):
    with io.open(path, "rb") as f:
        data = f.read()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    pos = 8
    idat = bytearray()
    meta = None
    palette = None
    while pos < len(data):
        length = struct.unpack(">I", data[pos:pos + 4])[0]
        tag = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + length]
        if tag == b"IHDR":
            w, h, bit, ctype, comp, filt, inter = struct.unpack(">IIBBBBB", body)
            assert bit == 8 and inter == 0, "unsupported PNG variant"
            meta = (w, h, ctype)
        elif tag == b"PLTE":
            palette = body
        elif tag == b"IDAT":
            idat += body
        elif tag == b"IEND":
            break
        pos += 12 + length
    w, h, ctype = meta
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[ctype]
    raw = zlib.decompress(bytes(idat))
    stride = w * channels
    out = bytearray(w * h * 4)
    prev = bytearray(stride)
    p = 0
    for y in range(h):
        ftype = raw[p]; p += 1
        line = bytearray(raw[p:p + stride]); p += stride
        if ftype == 1:  # Sub
            for i in range(channels, stride):
                line[i] = (line[i] + line[i - channels]) & 0xFF
        elif ftype == 2:  # Up
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif ftype == 3:  # Average
            for i in range(stride):
                a = line[i - channels] if i >= channels else 0
                line[i] = (line[i] + ((a + prev[i]) >> 1)) & 0xFF
        elif ftype == 4:  # Paeth
            for i in range(stride):
                a = line[i - channels] if i >= channels else 0
                b = prev[i]
                c = prev[i - channels] if i >= channels else 0
                pa = abs(b - c); pb = abs(a - c); pc = abs(a + b - 2 * c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 0xFF
        prev = line
        for x in range(w):
            o = (y * w + x) * 4
            if ctype == 6:
                out[o:o + 4] = line[x * 4:x * 4 + 4]
            elif ctype == 2:
                out[o:o + 3] = line[x * 3:x * 3 + 3]; out[o + 3] = 255
            elif ctype == 0:
                g = line[x]; out[o] = out[o + 1] = out[o + 2] = g; out[o + 3] = 255
            elif ctype == 4:
                out[o] = out[o + 1] = out[o + 2] = line[x * 2]; out[o + 3] = line[x * 2 + 1]
            elif ctype == 3:
                idx = line[x]
                out[o:o + 3] = palette[idx * 3:idx * 3 + 3]; out[o + 3] = 255
    return w, h, out

# ---------- 圆角遮罩 + 重编码 ----------

def inside_rounded_aa(x, y, n, r):
    """4×4 超采样的圆角覆盖率"""
    cnt = 0
    for sy in range(SS):
        for sx in range(SS):
            px = x + (sx + 0.5) / SS
            py = y + (sy + 0.5) / SS
            cx = min(max(px, r), n - r)
            cy = min(max(py, r), n - r)
            if (px - cx) ** 2 + (py - cy) ** 2 <= r * r:
                cnt += 1
    return cnt / (SS * SS)

def write_png(path, w, h, rgba):
    def chunk(tag, payload):
        c = struct.pack(">I", len(payload)) + tag + payload
        return c + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)
    stride = w * 4
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        raw += rgba[y * stride:(y + 1) * stride]
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b"")
    with io.open(path, "wb") as f:
        f.write(png)

w, h, px = read_png(SRC)
assert w == OUT and h == OUT, f"logo-src.png 应为 {OUT}x{OUT}"
r = CORNER
for y in range(OUT):
    # 仅在可能受圆角影响的边缘区域计算遮罩，加速中心区域
    near_edge = y < r or y >= OUT - r
    for x in range(OUT):
        o = (y * OUT + x) * 4
        if near_edge or x < r or x >= OUT - r:
            a = inside_rounded_aa(x, y, OUT, r)
            if a < 1.0:
                px[o + 3] = int(px[o + 3] * a)
here = os.path.dirname(os.path.abspath(__file__))
out = os.path.join(here, OUT_NAME)
write_png(out, OUT, OUT, px)
print("written", out, os.path.getsize(out), "bytes")

# 额外输出 UI 用小尺寸 logo（public/logo.png，供前端 <img> 引用）
UI = 128
step = OUT // UI
os.makedirs(os.path.join(here, "public"), exist_ok=True)
ui = bytearray()
for y in range(UI):
    for x in range(UI):
        r = g = b = a = 0
        for sy in range(step):
            for sx in range(step):
                o = ((y * step + sy) * OUT + (x * step + sx)) * 4
                pa = px[o + 3]
                r += px[o] * pa; g += px[o + 1] * pa; b += px[o + 2] * pa; a += pa
        if a == 0:
            ui += bytes(4)
        else:
            ui += bytes([r // a, g // a, b // a, a // (step * step)])
ui_out = os.path.join(here, "public", "logo.png")
write_png(ui_out, UI, UI, ui)
print("written", ui_out, os.path.getsize(ui_out), "bytes")
