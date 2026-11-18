//! The read side, on the 0.13 engine that wrote the draft Space: every
//! element in any storage state, as one selective Capsule, plus the facts a
//! Capsule import does not carry.
use db13::{
    database::{AndaDB, DBConfig},
    storage::StorageConfig,
};
use nexus13::{
    CognitiveNexus,
    governance::{AuthContext, EffectiveAuthority},
    kql::Context,
    nexus::DEFAULT_SPACE,
    store::Element,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::{path::Path, sync::Arc};

use crate::BoxError;

/// Values restored after import, including Concept identity and storage state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fact {
    pub id: String,
    pub kind: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub canonical_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schema_ref: String,
}

#[derive(Serialize, Deserialize)]
pub struct Exported {
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub space_created_at: String,
    pub capsule: Json,
    pub facts: Vec<Fact>,
    pub self_concept: String,
}

/// The storage settings the Anda Bot and Anda Brain hosts open a Space with.
pub fn storage_config() -> StorageConfig {
    StorageConfig {
        cache_max_capacity: 100000,
        cache_max_bytes: None,
        compress_level: 3,
        object_chunk_size: 256 * 1024,
        bucket_overload_size: 1024 * 1024,
        max_small_object_size: 1024 * 1024 * 10,
    }
}

pub async fn open_source(root: &Path, space: &str) -> Result<CognitiveNexus, BoxError> {
    let os = object_store::local::LocalFileSystem::new_with_prefix(root)?;
    let os = store13::MetaStoreBuilder::new(os, 100000).build();
    let db = Arc::new(
        AndaDB::open(
            Arc::new(os),
            DBConfig {
                name: space.to_string(),
                description: "Anda Brain database".into(),
                storage: storage_config(),
                lock: None,
            },
        )
        .await?,
    );
    Ok(CognitiveNexus::connect(db).await?)
}

pub async fn export(root: &Path, space: &str) -> Result<Exported, BoxError> {
    let nexus = open_source(root, space).await?;
    let store = &nexus.store;
    if store.spaces().len() != 1 {
        nexus.close().await?;
        return Err("migration requires a database containing only the default Nexus Space".into());
    }

    // Every element of the Space in id order, so an import into an empty
    // Nexus mints the same ids.
    let mut roots = Vec::new();
    let mut facts = Vec::new();
    let mut evidence = Vec::new();
    for (kind, prefix) in [
        (kip13::ElementKind::Concept, "C"),
        (kip13::ElementKind::Proposition, "P"),
        (kip13::ElementKind::Evidence, "E"),
        (kip13::ElementKind::Assertion, "A"),
        (kip13::ElementKind::Activity, "X"),
    ] {
        let mut ids = store.elements(kind).ids();
        ids.sort_unstable();
        for id in ids {
            let eid: nexus13::ElementId = format!("{prefix}-{id}").parse()?;
            let element = store.get_element(eid).await?;
            if element.space() != DEFAULT_SPACE {
                continue;
            }
            let (key, canonical_id) = match &element {
                Element::Concept(row) => (row.key.clone(), row.canonical_id.clone()),
                _ => (String::new(), String::new()),
            };
            if let Element::Evidence(row) = &element {
                // The exporter withholds Evidence payloads as unavailable;
                // the owner's own source material is carried as is.
                evidence.push(json!({
                    "id": eid.to_string(),
                    "kind": "evidence",
                    "space_id": row.space,
                    "evidence_class": row.evidence_class,
                    "payload": {
                        "mode": row.payload_mode,
                        "inline": row.payload_inline,
                        "content_ref": row.content_ref,
                    },
                    "content_digest": row.content_digest,
                    "media_type": row.media_type,
                    "observed_at": row.observed_at,
                    "source": row.source_refs,
                    "generated_by": if row.generated_by.is_empty() { Json::Null } else { json!({"id": row.generated_by}) },
                    "lifecycle": {
                        "status": row.status,
                        "corrects": row.corrects,
                        "corrected_by": row.corrected_by,
                    },
                    "facets": row.facets,
                    "structural": row.structural,
                    "governance": row.governance,
                    "_system": {"state": row.state},
                }));
            }
            facts.push(Fact {
                id: eid.to_string(),
                kind: prefix.to_string(),
                state: element.state().to_string(),
                key,
                canonical_id,
                schema_ref: element.schema_ref().to_string(),
            });
            roots.push(eid);
        }
    }

    let auth = AuthContext::system();
    let authority = EffectiveAuthority::resolve(store, DEFAULT_SPACE, &auth).await?;
    let mut cx = Context::open(store, DEFAULT_SPACE, None, None, &authority, &auth).await?;
    let mut options = serde_json::Map::new();
    options.insert("closure".into(), json!("selective"));
    let capsule = nexus13::capsule::export(&mut cx, roots, &options).await?;
    let mut capsule = serde_json::to_value(&capsule)?;

    // Carry the withheld Evidence in its id order among the records.
    let records = capsule["payload"]["records"]
        .as_array_mut()
        .ok_or("the exported Capsule has no records")?;
    let carried: std::collections::BTreeSet<String> = records
        .iter()
        .filter_map(|r| r["id"].as_str().map(str::to_string))
        .collect();
    records.extend(
        evidence
            .into_iter()
            .filter(|e| !carried.contains(e["id"].as_str().unwrap_or_default())),
    );
    if let Some(external) = capsule["payload"]["external_refs"].as_array_mut() {
        external.retain(|r| !r["id"].as_str().is_some_and(|id| id.starts_with("E-")));
    }

    let source = store.get_space(DEFAULT_SPACE).await?;
    // Closes the database too.
    nexus.close().await?;
    Ok(Exported {
        database: space.to_string(),
        space_created_at: source.created_at,
        capsule,
        facts,
        self_concept: source.self_concept,
    })
}
