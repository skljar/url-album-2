import ctypes, struct, zlib, time

u = ctypes.windll.user32
g = ctypes.windll.gdi32

# Find by title
hwnd = u.FindWindowW(None, "URL Album 3")
if not hwnd:
    # Try enumerating all windows
    found = []
    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_int, ctypes.c_int)
    def cb(h, _):
        buf = ctypes.create_unicode_buffer(256)
        u.GetWindowTextW(h, buf, 256)
        if 'Album' in buf.value or 'album' in buf.value.lower():
            found.append((h, buf.value))
        return True
    u.EnumWindows(cb, 0)
    print(f"Found windows: {found}")
    if found: hwnd = found[0][0]
    else: print("No window found"); exit(1)

print(f"hwnd: {hwnd}")
u.ShowWindow(hwnd, 9)
time.sleep(0.5)
u.SetForegroundWindow(hwnd)
time.sleep(1.0)

class R(ctypes.Structure):
    _fields_ = [("l",ctypes.c_long),("t",ctypes.c_long),("r",ctypes.c_long),("b",ctypes.c_long)]

rc = R(); u.GetWindowRect(hwnd, ctypes.byref(rc))
w, h = rc.r-rc.l, rc.b-rc.t
print(f"Size: {w}x{h}")

hdc = u.GetDC(0); hm = g.CreateCompatibleDC(hdc); hb = g.CreateCompatibleBitmap(hdc, w, h)
g.SelectObject(hm, hb); g.BitBlt(hm, 0, 0, w, h, hdc, rc.l, rc.t, 0x00CC0020)

class BIH(ctypes.Structure):
    _fields_ = [('sz',ctypes.c_uint32),('w',ctypes.c_int32),('h',ctypes.c_int32),
                ('pl',ctypes.c_uint16),('bc',ctypes.c_uint16),('comp',ctypes.c_uint32),
                ('si',ctypes.c_uint32),('x',ctypes.c_int32),('y',ctypes.c_int32),
                ('cu',ctypes.c_uint32),('ci',ctypes.c_uint32)]
bi = BIH(); bi.sz=40; bi.w=w; bi.h=-h; bi.pl=1; bi.bc=32
sz = w*h*4; buf = (ctypes.c_byte*sz)()
g.GetDIBits(hm, hb, 0, h, buf, ctypes.byref(bi), 0)

rows = []
for y in range(h):
    row = bytearray()
    for x in range(w):
        o=(y*w+x)*4; b,gr,r,a=buf[o]&255,buf[o+1]&255,buf[o+2]&255,buf[o+3]&255
        row+=bytes([r,gr,b])
    rows.append(bytes(row))

def chunk(n,d): c=zlib.crc32(n+d)&0xffffffff; return struct.pack('>I',len(d))+n+d+struct.pack('>I',c)
ihdr=struct.pack('>IIBBBBB',w,h,8,2,0,0,0)
idat=zlib.compress(b''.join(b'\x00'+r for r in rows))
png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',ihdr)+chunk(b'IDAT',idat)+chunk(b'IEND',b'')
out = r"C:\Projects\url-album-2\screenshots\ua3-final.png"
open(out,'wb').write(png)
print(f"Saved: {out}")
g.DeleteObject(hb); g.DeleteDC(hm); u.ReleaseDC(0,hdc)
