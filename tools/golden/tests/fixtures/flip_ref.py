"""Independent NumPy version of LDR-FLIP, written from NVlabs/flip src/pytorch/flip_loss.py
(BSD-3-Clause, NVIDIA) with full 2D kernels, to check the Rust port. Not run by cargo.

    python -I flip_ref.py

Prints the mean error of the test pair (picture 0 against picture 1) used in tests/metrics.rs.
"""
import numpy as np

PPD = (0.7 * 3840 / 0.7) * np.pi / 180
QC, QF, PC, PT, EPS = 0.7, 0.5, 0.4, 0.95, 1e-15
WHITE = np.array([0.950428545, 1.0, 1.088900371])
M_RGB2XYZ = np.array([
    [10135552 / 24577794, 8788810 / 24577794, 4435075 / 24577794],
    [2613072 / 12288897, 8788810 / 12288897, 887015 / 12288897],
    [1425312 / 73733382, 8788810 / 73733382, 70074185 / 73733382]])
M_XYZ2RGB = np.array([
    [3.241003275, -1.537398934, -0.498615861],
    [-0.969224334, 1.875930071, 0.041554224],
    [0.055639423, -0.204011202, 1.057148933]])


def picture(w, h, variant):
    img = np.zeros((h, w, 3), dtype=np.int64)
    for y in range(h):
        for x in range(w):
            c = [x * 255 // (w - 1), y * 255 // (h - 1), (x * 7 + y * 13) % 256]
            if x > w // 2 and y > h // 3:
                c = [200, 90, 30]
            if (x // 2 + y // 2) % 2 == 0 and x < w // 3:
                c = [c[0] // 2 + 30, c[1] // 2, c[2]]
            if variant == 1:
                c = [c[0] + 12, c[1] - 9, c[2] + 20]
                if y > h // 2 and x < w // 2:
                    c = [c[2], c[0], c[1]]
            img[y, x] = np.clip(c, 0, 255)
    return img.astype(np.float64) / 255.0


def srgb2lin(c):
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def srgb2ycxcz(img):
    xyz = srgb2lin(img) @ M_RGB2XYZ.T / WHITE
    y = 116 * xyz[..., 1] - 16
    cx = 500 * (xyz[..., 0] - xyz[..., 1])
    cz = 200 * (xyz[..., 1] - xyz[..., 2])
    return np.stack([y, cx, cz], -1)


def ycxcz2linrgb(p):
    yn = (p[..., 0] + 16) / 116
    xn = yn + p[..., 1] / 500
    zn = yn - p[..., 2] / 200
    xyz = np.stack([xn, yn, zn], -1) * WHITE
    return np.clip(xyz @ M_XYZ2RGB.T, 0, 1)


def linrgb2lab(rgb):
    xyz = rgb @ M_RGB2XYZ.T / WHITE
    d = 6 / 29
    f = np.where(xyz > d ** 3, np.cbrt(xyz), xyz / (3 * d * d) + 4 / 29)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]), 200 * (f[..., 1] - f[..., 2])], -1)


def hunt(lab):
    out = lab.copy()
    out[..., 1] = 0.01 * lab[..., 0] * lab[..., 1]
    out[..., 2] = 0.01 * lab[..., 0] * lab[..., 2]
    return out


def hyab(a, b):
    d = a - b
    return np.sqrt(np.maximum(d[..., 0] ** 2, EPS)) + np.linalg.norm(d[..., 1:], axis=-1)


def correlate2d_replicate(img, k):
    r = k.shape[0] // 2
    p = np.pad(img, r, mode='edge')
    h, w = img.shape
    out = np.zeros_like(img)
    for dy in range(k.shape[0]):
        for dx in range(k.shape[1]):
            out += k[dy, dx] * p[dy:dy + h, dx:dx + w]
    return out


def csf(a1, b1, a2, b2, r):
    ax = np.arange(-r, r + 1)
    x, y = np.meshgrid(ax, ax)
    z = (x / PPD) ** 2 + (y / PPD) ** 2
    g = a1 * np.sqrt(np.pi / b1) * np.exp(-np.pi ** 2 * z / b1)
    if a2 != 0:
        g = g + a2 * np.sqrt(np.pi / b2) * np.exp(-np.pi ** 2 * z / b2)
    return g / g.sum()


def feature_kernel(point):
    sd = 0.5 * 0.082 * PPD
    r = int(np.ceil(3 * sd))
    ax = np.arange(-r, r + 1)
    x, y = np.meshgrid(ax, ax)
    g = np.exp(-(x ** 2 + y ** 2) / (2 * sd * sd))
    gx = (x ** 2 / (sd * sd) - 1) * g if point else -x * g
    neg = -gx[gx < 0].sum()
    pos = gx[gx > 0].sum()
    return np.where(gx < 0, gx / neg, gx / pos)


def flip(ref, test):
    bs = [0.0047, 1e-5, 0.0053, 1e-5, 0.04, 0.025]
    r = int(np.ceil(3 * np.sqrt(max(bs) / (2 * np.pi ** 2)) * PPD))
    kernels = [csf(1, 0.0047, 0, 1e-5, r), csf(1, 0.0053, 0, 1e-5, r), csf(34.1, 0.04, 13.5, 0.025, r)]
    yr, yt = srgb2ycxcz(ref), srgb2ycxcz(test)

    def filt(p):
        return np.stack([correlate2d_replicate(p[..., c], kernels[c]) for c in range(3)], -1)

    lr = hunt(linrgb2lab(ycxcz2linrgb(filt(yr))))
    lt = hunt(linrgb2lab(ycxcz2linrgb(filt(yt))))
    de = hyab(lr, lt) ** QC
    cmax = hyab(hunt(linrgb2lab(np.array([0., 1., 0.]))), hunt(linrgb2lab(np.array([0., 0., 1.])))) ** QC
    pcc = PC * cmax
    dec = np.where(de < pcc, (PT / pcc) * de, PT + ((de - pcc) / (cmax - pcc)) * (1 - PT))

    ke, kp = feature_kernel(False), feature_kernel(True)

    def norms(yn, k):
        fx = correlate2d_replicate(yn, k)
        fy = correlate2d_replicate(yn, k.T)
        return np.sqrt(fx ** 2 + fy ** 2)

    nr, nt = (yr[..., 0] + 16) / 116, (yt[..., 0] + 16) / 116
    dfe = np.maximum(np.abs(norms(nr, ke) - norms(nt, ke)), np.abs(norms(nt, kp) - norms(nr, kp)))
    dfe = np.maximum(dfe, EPS)
    dfe = ((1 / np.sqrt(2)) * dfe) ** QF
    return dec ** (1 - dfe)


if __name__ == '__main__':
    a, b = picture(64, 48, 0), picture(64, 48, 1)
    print('mean a vs b: %.6f' % flip(a, b).mean())
    print('mean a vs a: %.8f' % flip(a, a).mean())
