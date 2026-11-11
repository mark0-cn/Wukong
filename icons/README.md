<a id="wukong-图标"></a>

# Wukong icons

English · [简体中文](README.zh-CN.md)

`icon.svg` is the sole design source. Deep ink-green reflects the workspace; inward gold headband curls, the Monkey King mask and orange eye slits express Wukong, recognition and insight. Symmetric geometry/negative space preserve headband/face recognition at small sizes without text, thin lines or complex textures.

Sidebar/welcome views use the same SVG. The locked Tauri CLI generates Windows ICO, macOS ICNS and PNG sizes:

```powershell
./scripts/dev/generate-icons.ps1
```

Install root npm dependencies first. Intermediates live in ignored `target/generated-icons`; only desktop assets are updated, not mobile icons. After design changes, check 32px, 64px and in-app rendering and regenerate every desktop asset.
