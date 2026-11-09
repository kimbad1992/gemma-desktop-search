"""앱 아이콘 생성 (Pillow). 실행: python assets/make_icons.py

- icon.png      256px, 창 아이콘
- icon.ico      16~256px, exe 리소스
- tray-busy.ico  인덱싱 중 (초록 점)
- tray-paused.ico 일시정지 (주황 점 + 막대)
디자인: 남색→청록 그라데이션 둥근 사각형 위에 흰 돋보기, 렌즈 안에 반짝임(의미 검색).
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter

HERE = Path(__file__).resolve().parent
S = 1024  # 크게 그린 뒤 줄여서 계단 현상 제거


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def base():
    top, bottom = (44, 62, 145), (18, 164, 160)
    grad = Image.new('RGB', (S, S))
    px = grad.load()
    for y in range(S):
        for x in range(S):
            t = (x + y) / (2 * (S - 1))
            px[x, y] = lerp(top, bottom, t)
    mask = Image.new('L', (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle([24, 24, S - 24, S - 24], radius=220, fill=255)
    img = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)
    # 돋보기: 렌즈 고리 + 손잡이
    cx, cy, r, w = 430, 430, 230, 84
    d.ellipse([cx - r, cy - r, cx + r, cy + r], outline=(255, 255, 255, 255), width=w)
    # 손잡이 (45도)
    import math
    a = math.radians(45)
    x0 = cx + (r - 10) * math.cos(a)
    y0 = cy + (r - 10) * math.sin(a)
    x1, y1 = 820, 820
    d.line([x0, y0, x1, y1], fill=(255, 255, 255, 255), width=118)
    d.ellipse([x1 - 59, y1 - 59, x1 + 59, y1 + 59], fill=(255, 255, 255, 255))
    # 렌즈 안 반짝임 (네 갈래 별)
    def sparkle(x, y, R, rr, col):
        pts = []
        for i in range(8):
            ang = math.radians(i * 45 - 90)
            rad = R if i % 2 == 0 else rr
            pts.append((x + rad * math.cos(ang), y + rad * math.sin(ang)))
        d.polygon(pts, fill=col)
    sparkle(cx, cy, 120, 30, (255, 236, 140, 255))
    sparkle(cx + 95, cy - 95, 48, 12, (255, 255, 255, 235))
    return img


def badge(img, color, pause=False):
    img = img.copy()
    d = ImageDraw.Draw(img)
    R = 190
    cx, cy = S - R - 10, S - R - 10
    d.ellipse([cx - R - 26, cy - R - 26, cx + R + 26, cy + R + 26], fill=(20, 24, 40, 255))
    d.ellipse([cx - R, cy - R, cx + R, cy + R], fill=color)
    if pause:
        bw, bh = 52, 190
        d.rounded_rectangle([cx - 85, cy - bh // 2, cx - 85 + bw, cy + bh // 2], radius=14, fill=(255, 255, 255, 255))
        d.rounded_rectangle([cx + 33, cy - bh // 2, cx + 33 + bw, cy + bh // 2], radius=14, fill=(255, 255, 255, 255))
    return img


def save_ico(img, path, sizes):
    small = img.filter(ImageFilter.SMOOTH)
    small.save(path, format='ICO', sizes=[(s, s) for s in sizes])


def main():
    img = base()
    img.resize((256, 256), Image.LANCZOS).save(HERE / 'icon.png')
    save_ico(img, HERE / 'icon.ico', [16, 20, 24, 32, 40, 48, 64, 128, 256])
    save_ico(badge(img, (46, 204, 113, 255)), HERE / 'tray-busy.ico', [16, 20, 24, 32, 48])
    save_ico(badge(img, (243, 156, 18, 255), pause=True), HERE / 'tray-paused.ico', [16, 20, 24, 32, 48])
    # 미리보기 (README용)
    preview = Image.new('RGBA', (3 * 140, 140), (0, 0, 0, 0))
    for i, im in enumerate([img, badge(img, (46, 204, 113, 255)), badge(img, (243, 156, 18, 255), True)]):
        preview.paste(im.resize((128, 128), Image.LANCZOS), (i * 140 + 6, 6))
    preview.save(HERE / 'icon-preview.png')
    print('ok')


if __name__ == '__main__':
    main()
