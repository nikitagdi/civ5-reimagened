//! The game art of the page's own screens (web lobby and room, P15): the `web:lobby` table of the asset set
//! (`tools/asset-pipeline` set `web-lobby`: the `Colors` table, the icon-font glyph rects, and per civilization the
//! leader portrait, the civ icon and the unique units, buildings and improvements) and the URLs of its PNG
//! sheets. Once the manifest is known the table is fetched, every `"img"` logical id in it is resolved to the
//! blob's URL (`<assets base><hash>`), and the page gets it once as the screen `"art"` of the bridge
//! (`ui.js`: `{"urls": {id: url}, "art": <table>}`). Without the entry (an older asset set) the page keeps its
//! plain text.

use std::collections::BTreeMap;

use assets::{AssetHandle, AssetLoader, AssetPoll, LogicalId, PlatformProfile};

/// Manifest id of the table.
pub(crate) const LOBBY_ART_ID: &str = "web:lobby";

enum Stage {
    Waiting,
    Loading(AssetHandle),
    Done,
}

pub(crate) struct LobbyArt {
    /// `http://host/assets/` (with the slash), or `None`: no assets, nothing to do.
    base: Option<String>,
    stage: Stage,
}

impl LobbyArt {
    pub(crate) fn new(base: Option<String>) -> Self {
        Self {
            base,
            stage: Stage::Waiting,
        }
    }

    /// Advances the loading; returns the JSON for the page once, when the table has arrived.
    pub(crate) fn update(&mut self, loader: &mut AssetLoader) -> Option<String> {
        let base = self.base.as_deref()?;
        let profile = PlatformProfile::uncompressed_only();
        match &self.stage {
            Stage::Done => None,
            Stage::Waiting => {
                // `AssetsBoot` drives the manifest; this only looks.
                if loader.manifest_error().is_some() {
                    self.stage = Stage::Done;
                    return None;
                }
                let manifest = loader.manifest()?;
                let id = LogicalId::new(LOBBY_ART_ID);
                if manifest.resolve(&id, &profile).is_none() {
                    tracing::info!(
                        "the asset set has no {LOBBY_ART_ID}: the lobby shows plain text"
                    );
                    self.stage = Stage::Done;
                    return None;
                }
                self.stage = Stage::Loading(loader.request(id, &profile));
                None
            }
            Stage::Loading(h) => match loader.poll(*h) {
                AssetPoll::Pending => None,
                AssetPoll::Failed(e) => {
                    tracing::warn!("{LOBBY_ART_ID} unavailable ({e})");
                    self.stage = Stage::Done;
                    None
                }
                AssetPoll::Ready(bytes) => {
                    self.stage = Stage::Done;
                    let manifest = loader.manifest()?;
                    let out = page_json(&bytes, base, |id| {
                        manifest
                            .resolve(&LogicalId::new(id), &profile)
                            .map(|v| v.hash.to_string())
                    });
                    if out.is_none() {
                        tracing::warn!("{LOBBY_ART_ID} is not the expected JSON");
                    }
                    out
                }
            },
        }
    }
}

/// Every string under an `"img"` key, anywhere in `v`.
fn collect_imgs(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, x) in m {
                match (k.as_str(), x) {
                    ("img", serde_json::Value::String(s)) => out.push(s.clone()),
                    _ => collect_imgs(x, out),
                }
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_imgs(x, out)),
        _ => {}
    }
}

/// `{"urls": {img id: url}, "art": table}`; an id the manifest does not know gets no URL (the page shows no
/// picture for it). `None` when `bytes` are not a JSON object.
fn page_json(bytes: &[u8], base: &str, hash_of: impl Fn(&str) -> Option<String>) -> Option<String> {
    let art: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if !art.is_object() {
        return None;
    }
    let mut imgs = Vec::new();
    collect_imgs(&art, &mut imgs);
    let urls: BTreeMap<String, String> = imgs
        .into_iter()
        .filter_map(|id| hash_of(&id).map(|h| (id, format!("{base}{h}"))))
        .collect();
    serde_json::to_string(&serde_json::json!({ "urls": urls, "art": art })).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_img_gets_the_url_of_its_blob() {
        let table = br#"{"version":1,"colors":{"COLOR_RED":[255,0,0,255]},
            "icons":{"ICON_GOLD":{"img":"web:png:fonticons-0","x":1,"y":2,"w":22,"h":22}},
            "civs":{"CIVILIZATION_ROME":{"leader":{"type":"LEADER_AUGUSTUS","name":"Augustus","cell":{"img":"web:png:civ-civilization_rome","x":0,"y":0,"w":128,"h":128}},
            "icon":null,"uniques":[{"kind":"Unit","type":"UNIT_LEGION","name":"Legion","cell":{"img":"web:png:missing","x":0,"y":0,"w":64,"h":64}}]}}}"#;
        let out = page_json(table, "http://h/assets/", |id| {
            (id != "web:png:missing").then(|| format!("hash-{id}"))
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["urls"]["web:png:fonticons-0"],
            "http://h/assets/hash-web:png:fonticons-0"
        );
        assert_eq!(
            v["urls"]["web:png:civ-civilization_rome"],
            "http://h/assets/hash-web:png:civ-civilization_rome"
        );
        assert!(v["urls"].get("web:png:missing").is_none());
        assert_eq!(v["art"]["colors"]["COLOR_RED"][0], 255);
    }

    #[test]
    fn a_table_that_is_not_an_object_is_refused() {
        assert!(page_json(b"[1,2]", "b/", |_| None).is_none());
        assert!(page_json(b"not json", "b/", |_| None).is_none());
    }
}
