//! 皮肤解码：把 `zouwu-platform::assets::SkinPackage`（清单 + PNG 字节）
//! 变成可以直接画的 `Skin`（已解码的 Pixmap + 扁平化的几何参数）。
//!
//! 为什么解码放在 `zouwu-ui` 而不是 `assets`：解码结果就是 tiny-skia 的
//! `Pixmap`，那是渲染层的类型。`assets` 只认字节，不认像素，这样它不需要
//! 依赖 tiny-skia，也不需要知道 DPI。
//!
//! 解码是**一次性成本**（启动 / 切皮肤时各一次），之后每帧只是把 Pixmap
//! 按变换 blit 上去。位图皮肤确实比代码绘制便宜，但**没有数量级差距**——
//! 一开始我以为是“快一个量级”，实测后被证伪了：
//!
//! | 路径 | 单帧（340² 画布，展开态，release） | 相对 |
//! |---|---|---|
//! | 代码绘制（procedural） | 1092 µs | 1.00× |
//! | 位图皮肤 112² → 112 px（1:1） | 687 µs | **1.59×** |
//! | 位图皮肤 256² → 112 px（缩放） | 754 µs | 1.45× |
//!
//! 三条结论：
//! - 位图赢在“不再做 12 层矢量阴影 + 十几个圆”，输在每像素一次 bilinear 采样；
//! - **按实际显示尺寸导出 1x 图最快**，导出 2x/4x 大图反而慢 10%（缩放不是免费）；
//! - 机器噪声很大（同段代码两次运行可差 1.5×），所以 `skin_demo` 用
//!   “轮流各跑 120 帧 × 6 轮取最小值”来量，而不是顺序地量 A 再量 B。
//!
//! 复现：`cargo run --release -p zouwu-ui --example skin_demo`（同时导出四张验收图）。
//!
//! 校验门槛（都在解码前/后各一道）：
//! - 尺寸 1..=[`MAX_LAYER_SIDE`]，挡住"20000² 的解压炸弹"；
//! - 非空皮肤至少要有一层 `body`，否则画出来是空气；
//! - 单张图层解码失败 → 整包失败（宁可退回 procedural，也不要画半个人）。

use tiny_skia::Pixmap;
use zouwu_core::skin::{LayerRole, SkinManifest};
use zouwu_platform::assets::SkinPackage;

/// 单边像素上限。2048² 的贴图缩放后已经足够 4K 屏用。
pub const MAX_LAYER_SIDE: u32 = 4096;

#[derive(Debug)]
pub struct SkinLayer {
    pub role: LayerRole,
    /// 相对皮肤设计尺寸的倍数（1.0 = 长边恰好等于 `2 × geometry.radius`）。
    pub scale: f32,
    /// 逻辑像素偏移（正值 = 右下）。
    pub offset_x: f32,
    pub offset_y: f32,
    pub opacity: f32,
    pub pixmap: Pixmap,
}

impl SkinLayer {
    /// 这一层按长边归一后应该画多大（逻辑像素）。
    ///
    /// 规则只有一条：**长边 = `2 × actual_radius × scale`**，等比缩放。
    /// 这样高低分辨率的同一张皮肤上下采后一模一样，作者不必关心设备 DPI。
    pub fn draw_size(&self, actual_radius: f32) -> (f32, f32) {
        let longest = self.pixmap.width().max(self.pixmap.height()) as f32;
        if longest <= 0.0 {
            return (0.0, 0.0);
        }
        let k = actual_radius * 2.0 * self.scale / longest;
        (
            self.pixmap.width() as f32 * k,
            self.pixmap.height() as f32 * k,
        )
    }
}

#[derive(Debug)]
pub struct Skin {
    pub manifest: SkinManifest,
    pub layers: Vec<SkinLayer>,
}

impl Skin {
    /// 内置的"代码绘制"皮肤：没有任何图层，渲染时走 `paint::draw_buddy`。
    pub fn procedural() -> Self {
        Self {
            manifest: SkinManifest::procedural(),
            layers: Vec::new(),
        }
    }

    pub fn decode(package: &SkinPackage) -> Result<Self, String> {
        let manifest = package.manifest.clone();
        let mut layers = Vec::with_capacity(package.images.len());

        for layer in &manifest.layers {
            let Some((_, bytes)) = package.images.iter().find(|(name, _)| name == &layer.image)
            else {
                return Err(format!(
                    "layer `{}`: bytes missing from package",
                    layer.image
                ));
            };
            let pixmap = Pixmap::decode_png(bytes)
                .map_err(|e| format!("layer `{}` is not a readable PNG: {e}", layer.image))?;
            let (w, h) = (pixmap.width(), pixmap.height());
            if w == 0 || h == 0 || w > MAX_LAYER_SIDE || h > MAX_LAYER_SIDE {
                return Err(format!(
                    "layer `{}` is {w}×{h}, outside 1..={MAX_LAYER_SIDE}",
                    layer.image
                ));
            }
            layers.push(SkinLayer {
                role: layer.role,
                scale: layer.scale,
                offset_x: layer.offset_x,
                offset_y: layer.offset_y,
                opacity: layer.opacity,
                pixmap,
            });
        }

        if !layers.is_empty() && !layers.iter().any(|l| l.role == LayerRole::Body) {
            return Err(format!(
                "skin `{}` has no `body` layer — nothing would be visible",
                manifest.id
            ));
        }

        Ok(Self { manifest, layers })
    }

    /// true = 用代码画（没有位图图层）。
    pub fn is_procedural(&self) -> bool {
        self.layers.is_empty()
    }

    pub fn layers_of(&self, role: LayerRole) -> impl Iterator<Item = &SkinLayer> {
        self.layers.iter().filter(move |l| l.role == role)
    }

    /// 呼吸起伏幅度（来自皮肤清单，皮肤作者可以让自家形象喘得更明显/更轻）。
    pub fn breath_amount(&self) -> f32 {
        if self.is_procedural() {
            crate::paint::DEFAULT_BREATH_AMOUNT
        } else {
            self.manifest.geometry.breath_amount
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zouwu_core::skin::SkinManifest;

    fn blank_png(w: u32, h: u32) -> Vec<u8> {
        let mut pixmap = Pixmap::new(w, h).expect("pixmap");
        pixmap.fill(tiny_skia::Color::from_rgba8(200, 120, 80, 255));
        pixmap.encode_png().expect("png")
    }

    fn package(manifest: &str, images: Vec<(&str, Vec<u8>)>) -> SkinPackage {
        SkinPackage {
            manifest: SkinManifest::parse(manifest).expect("manifest"),
            dir: std::path::PathBuf::from("/nowhere"),
            images: images
                .into_iter()
                .map(|(n, b)| (n.to_string(), b))
                .collect(),
        }
    }

    #[test]
    fn procedural_has_no_layers() {
        let skin = Skin::procedural();
        assert!(skin.is_procedural());
        assert_eq!(skin.breath_amount(), crate::paint::DEFAULT_BREATH_AMOUNT);
    }

    #[test]
    fn decodes_png_layers_in_order() {
        let pkg = package(
            "id = \"bitmap\"\n[[layers]]\nimage = \"body.png\"\n[[layers]]\nimage = \"eyes.png\"\nrole = \"blink\"",
            vec![
                ("body.png", blank_png(64, 64)),
                ("eyes.png", blank_png(32, 16)),
            ],
        );
        let skin = Skin::decode(&pkg).expect("decode");
        assert!(!skin.is_procedural());
        assert_eq!(skin.layers.len(), 2);
        assert_eq!(skin.layers[0].role, LayerRole::Body);
        assert_eq!(skin.layers_of(LayerRole::Blink).count(), 1);
        assert_eq!(skin.layers[1].pixmap.width(), 32);
    }

    #[test]
    fn rejects_missing_or_broken_png() {
        let pkg = package(
            "id = \"bitmap\"\n[[layers]]\nimage = \"body.png\"",
            vec![("body.png", b"not a png".to_vec())],
        );
        let err = Skin::decode(&pkg).expect_err("must fail");
        assert!(err.contains("readable PNG"), "err was: {err}");

        let pkg = package("id = \"bitmap\"\n[[layers]]\nimage = \"body.png\"", vec![]);
        let err = Skin::decode(&pkg).expect_err("must fail");
        assert!(err.contains("missing"), "err was: {err}");
    }

    #[test]
    fn rejects_skin_without_body() {
        let pkg = package(
            "id = \"bitmap\"\n[[layers]]\nimage = \"eyes.png\"\nrole = \"overlay\"",
            vec![("eyes.png", blank_png(8, 8))],
        );
        let err = Skin::decode(&pkg).expect_err("must fail");
        assert!(err.contains("no `body` layer"), "err was: {err}");
    }

    #[test]
    fn draw_size_normalises_to_design_radius() {
        let pkg = package(
            "id = \"bitmap\"\n[geometry]\nradius = 56.0\n[[layers]]\nimage = \"body.png\"",
            vec![("body.png", blank_png(200, 100))],
        );
        let skin = Skin::decode(&pkg).expect("decode");
        let (w, h) = skin.layers[0].draw_size(56.0);
        // 长边 = 2 × radius = 112，等比 → 112 × 56
        assert!((w - 112.0).abs() < 0.01, "w was {w}");
        assert!((h - 56.0).abs() < 0.01, "h was {h}");
    }
}
