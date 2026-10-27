//! 皮肤清单（`skin.toml`）。
//!
//! 皮肤是**纯数据**：一组 PNG 图层 + 一点几何参数，没有任何可执行内容。
//! 这样皮肤可以随手分享、可以被静态检查、可以在没有改代码的情况下换成任何形象
//! （包括非驺吾的角色）。
//!
//! 与渲染的分工：
//! - 本模块只负责**解析与校验**（不碰文件系统、不碰像素）；
//! - `zouwu-platform::assets` 负责按安全路径读出 `skin.toml` 与图片字节；
//! - `zouwu-ui::skin` 负责解码成 `Pixmap`；
//! - `zouwu-ui::paint` 决定每个 `LayerRole` 什么时候画。
//!
//! 当 `layers` 为空时，皮肤就是 `procedural`——用代码画出来的那只小兽，
//! 好处是"没有任何皮肤文件时界面依然完整"（也是外部皮肤全坏时的兜底）。

use serde::{Deserialize, Serialize};

pub const SKIN_SCHEMA_VERSION: u32 = 1;

/// `procedural` 是内置的"代码绘制"皮肤，不对应磁盘上的目录。
pub const PROCEDURAL_SKIN_ID: &str = "procedural";

/// 图层的职责决定了它跟随哪个动画。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerRole {
    /// 身体：跟随呼吸起伏、入场上浮、整体缩放与透明度。
    #[default]
    Body,
    /// 闭眼图：**只在眨眼时**叠加（默认透明度随眨眼进度淡入淡出）。
    Blink,
    /// 装饰：始终画在身体之上，跟随同样的变换（围巾、光点、署名等）。
    Overlay,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    /// 相对皮肤目录的 PNG 路径，如 `body.png` 或 `parts/blink.png`。
    pub image: String,
    #[serde(default)]
    pub role: LayerRole,
    /// 该层相对整体尺寸的缩放（1.0 = 原图尺寸按 `geometry.radius` 归一）。
    #[serde(default = "one")]
    pub scale: f32,
    /// 该层在皮肤画布内的偏移（逻辑像素，正值=右/下）。
    #[serde(default)]
    pub offset_x: f32,
    #[serde(default)]
    pub offset_y: f32,
    /// 该层整体透明度（0..=1）。
    #[serde(default = "one")]
    pub opacity: f32,
}

fn one() -> f32 {
    1.0
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SkinGeometry {
    /// 形象半径（逻辑像素）：决定命中区域、气泡环的内半径参照。
    pub radius: f32,
    /// 图片锚点：HTML 里是提示气泡，**这里的 0.5/0.5 = 图片中心对准小兽中心**。
    pub anchor_x: f32,
    pub anchor_y: f32,
    /// 呼吸缩放幅度（0.03 = ±3%），0 关闭。
    pub breath_amount: f32,
}

impl Default for SkinGeometry {
    fn default() -> Self {
        Self {
            radius: 56.0,
            anchor_x: 0.5,
            anchor_y: 0.5,
            breath_amount: 0.03,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkinManifest {
    #[serde(default)]
    pub schema_version: u32,
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// 推荐主题 id（切皮肤时顺手切配色，但主题仍可独立覆盖）。
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub geometry: SkinGeometry,
    #[serde(default)]
    pub layers: Vec<Layer>,
}

impl SkinManifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let manifest: Self = toml::from_str(text).map_err(|err| format!("skin.toml: {err}"))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// 导出成可再次导入的 `skin.toml`（`skin_demo --install` 用它写盘）。
    ///
    /// 只序列化清单本身：图片路径原样保留，由调用方负责把 PNG 放到同位。
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_else(|_| String::new())
    }

    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    /// 没有图层的皮肤 = 代码绘制（内置 `procedural`）。
    pub fn is_procedural(&self) -> bool {
        self.layers.is_empty()
    }

    pub fn procedural() -> Self {
        Self {
            schema_version: SKIN_SCHEMA_VERSION,
            id: PROCEDURAL_SKIN_ID.to_string(),
            name: Some("驺吾 · 代码绘制".to_string()),
            author: Some("zouwu".to_string()),
            version: None,
            theme: None,
            geometry: SkinGeometry::default(),
            layers: Vec::new(),
        }
    }

    pub fn role_layers(&self, role: LayerRole) -> impl Iterator<Item = &Layer> {
        self.layers.iter().filter(move |l| l.role == role)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("skin id must not be empty".into());
        }
        // id 同时是目录名：别让它带路径分隔符或 `..`，否则加载器会被指到皮肤目录外。
        if self.id.contains(['/', '\\', ':']) || self.id.contains("..") {
            return Err(format!("skin id `{}` must be a plain name", self.id));
        }
        if self.schema_version > SKIN_SCHEMA_VERSION {
            return Err(format!(
                "skin `{}` needs schema_version {}, this build supports {} — please update zouwu",
                self.id, self.schema_version, SKIN_SCHEMA_VERSION
            ));
        }

        let g = self.geometry;
        if !(4.0..=512.0).contains(&g.radius) {
            return Err(format!(
                "geometry.radius = {} is out of range (4..=512)",
                g.radius
            ));
        }
        if !(0.0..=1.0).contains(&g.anchor_x) || !(0.0..=1.0).contains(&g.anchor_y) {
            return Err("geometry.anchor_* must be within 0..=1".into());
        }
        if !(0.0..=0.25).contains(&g.breath_amount) {
            return Err(format!(
                "geometry.breath_amount = {} is out of range (0..=0.25)",
                g.breath_amount
            ));
        }

        for (i, layer) in self.layers.iter().enumerate() {
            if !layer.image.to_ascii_lowercase().ends_with(".png") {
                return Err(format!(
                    "layers[{i}].image = `{}` must reference a .png",
                    layer.image
                ));
            }
            if is_unsafe_relative(&layer.image) {
                return Err(format!(
                    "layers[{i}].image = `{}` must stay inside the skin folder",
                    layer.image
                ));
            }
            if !(0.05..=8.0).contains(&layer.scale) {
                return Err(format!(
                    "layers[{i}].scale = {} is out of range (0.05..=8)",
                    layer.scale
                ));
            }
            if !(0.0..=1.0).contains(&layer.opacity) {
                return Err(format!(
                    "layers[{i}].opacity = {} is out of range (0..=1)",
                    layer.opacity
                ));
            }
        }
        Ok(())
    }
}

/// 目录穿越的一票否决检查：绝对路径、盘符、`..`、反斜杠、空路径。
pub fn is_unsafe_relative(path: &str) -> bool {
    let p = path.trim();
    if p.is_empty() || p.contains('\\') || p.contains(':') {
        return true;
    }
    if p.starts_with('/') {
        return true;
    }
    p.split('/').any(|seg| seg == ".." || seg.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procedural_has_no_layers() {
        let skin = SkinManifest::procedural();
        assert!(skin.is_procedural());
        assert_eq!(skin.geometry.radius, 56.0);
        assert!(skin.validate().is_ok());
    }

    #[test]
    fn minimal_manifest_gets_defaults() {
        let skin = SkinManifest::parse("id = \"mine\"").expect("valid");
        assert_eq!(skin.display_name(), "mine");
        assert_eq!(skin.geometry.radius, 56.0);
        assert_eq!(skin.geometry.anchor_y, 0.5);
        assert!(skin.is_procedural());
    }

    #[test]
    fn layers_keep_order_and_roles() {
        let skin = SkinManifest::parse(
            r#"
id = "painted"
name = "手绘驺吾"
[geometry]
radius = 72.0
[[layers]]
image = "body.png"
[[layers]]
image = "parts/blink.png"
role = "blink"
opacity = 0.8
"#,
        )
        .expect("valid");
        assert!(!skin.is_procedural());
        assert_eq!(skin.layers.len(), 2);
        assert_eq!(skin.layers[0].role, LayerRole::Body);
        assert_eq!(skin.layers[1].role, LayerRole::Blink);
        assert_eq!(skin.layers[1].opacity, 0.8);
        assert_eq!(skin.layers[0].scale, 1.0);
        assert_eq!(skin.geometry.radius, 72.0);
        assert_eq!(skin.role_layers(LayerRole::Blink).count(), 1);
    }

    #[test]
    fn rejects_path_traversal() {
        for bad in [
            "../evil.png",
            "/abs.png",
            "C:/x.png",
            "a\\b.png",
            "parts/../../x.png",
        ] {
            // TOML 单引号字面量：`a\b.png` 里的反斜杠要原样留住，
            // 不然测的就是“转义解析”而不是“路径穿越”。
            let text = format!("id = \"s\"\n[[layers]]\nimage = '{bad}'");
            assert!(
                SkinManifest::parse(&text).is_err(),
                "`{bad}` should be rejected"
            );
        }
    }

    #[test]
    fn rejects_silly_ids() {
        for bad in ["", "..", "a/b"] {
            let text = format!("id = \"{bad}\"");
            assert!(SkinManifest::parse(&text).is_err(), "`{bad}` should fail");
        }
    }

    #[test]
    fn rejects_future_schema() {
        let err = SkinManifest::parse("schema_version = 99\nid = \"x\"").expect_err("must fail");
        assert!(err.contains("update zouwu"), "err was: {err}");
    }

    #[test]
    fn rejects_out_of_range_numbers() {
        assert!(SkinManifest::parse("id = \"x\"\n[geometry]\nradius = 0.0").is_err());
        assert!(SkinManifest::parse("id = \"x\"\n[geometry]\nanchor_x = 2.0").is_err());
        assert!(
            SkinManifest::parse("id = \"x\"\n[[layers]]\nimage = \"a.png\"\nopacity = 3.0")
                .is_err()
        );
    }

    #[test]
    fn unknown_role_is_an_error_not_a_crash() {
        let err = SkinManifest::parse("id = \"x\"\n[[layers]]\nimage = \"a.png\"\nrole = \"tail\"")
            .expect_err("unknown role must fail");
        assert!(err.contains("skin.toml"), "err was: {err}");
    }

    #[test]
    fn unsafe_relative_detection() {
        assert!(!is_unsafe_relative("body.png"));
        assert!(!is_unsafe_relative("parts/eyes/blink.png"));
        assert!(is_unsafe_relative("//server/share/x.png"));
        assert!(is_unsafe_relative("a//b.png"));
        assert!(is_unsafe_relative(""));
    }
}
