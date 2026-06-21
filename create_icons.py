#!/usr/bin/env python3
import os

# 最小的有效 1x1 蓝色 PNG（Base64 解码后）
png_hex = bytes.fromhex(
    '89504e470d0a1a0a0000000d49484452000000200000002008060000007'
    '34a7e4300000006624b474400ff00ff00ffa0bda7930000000970485973'
    '0000000b0000000b01b8aa38ec0000001049444154485763f8cfc00000'
    '00000000000003ffffff7f00ff0000007de29b7f0000000049454e4442'
    '60826082'
)

os.makedirs('src-tauri/icons', exist_ok=True)
os.chdir('src-tauri/icons')

for name in ['32x32.png', '128x128.png', '128x128@2x.png', 'icon.png']:
    with open(name, 'wb') as f:
        f.write(png_hex)

print('图标已创建')
