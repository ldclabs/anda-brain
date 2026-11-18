//! The write side, on the KIP 2.0 engine: the Space's Nexus is rebuilt from
//! nothing inside the same database, so no draft artifact, draft Schema
//! Environment or element version survives, while the host's own collections
//! (conversations, ledgers, wiki) and extensions stay untouched.
use anda_cognitive_nexus::{
    CognitiveNexus,
    capsule::SymbolMapping,
    governance::{AuthContext, EffectiveAuthority},
    nexus::DEFAULT_SPACE,
    schema::{PackageState, SchemaLock, SchemaPackage},
    store::{Element, planes::PlaneKey, space::JournalEntry},
    tx::{Guard, Transaction},
};
use anda_db::database::{AndaDB, DBConfig};
use anda_kip::{Executor, Request};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

use crate::{BoxError, export::Exported};

const DRAFT_PROFILE: &str = "kip://profiles/cognitive-memory@2.1.0/";
const PROFILE: &str = "kip://profiles/cognitive-memory@2.0.0/";
const PROFILE_ID: &str = "kip://profiles/cognitive-memory";
const DRAFT_VOCABULARY: &str = "kip://local/draft@0.0.0/";

/// The collections a Cognitive Nexus owns. Everything else in the database
/// belongs to the host and is kept.
const NEXUS_COLLECTIONS: &[&str] = &[
    "concepts",
    "propositions",
    "assertions",
    "evidence",
    "activities",
    "spaces",
    "transactions",
    "schema_packages",
    "schema_envs",
    "element_versions",
    "kip_control_records",
    "kip_commit_log",
    "kip_exposures",
    "kip_legacy_v1",
    "gov_principals",
    "gov_principal_groups",
    "gov_actor_bindings",
    "gov_grants",
    "gov_delegations",
    "gov_policies",
    "gov_approvals",
    "gov_audit",
];

/// The option type one draft-typed Concept becomes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OptionType {
    #[serde(rename = "type")]
    pub name: String,
    pub description: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub dropped_collections: Vec<String>,
    pub kept_collections: Vec<String>,
    pub packages: Vec<String>,
    pub defined_types: Vec<String>,
    pub imported: BTreeMap<String, usize>,
    pub id_changes: Vec<(String, String)>,
    pub lineage_activities: Vec<String>,
    pub keys_restored: usize,
    pub archived_restored: usize,
    pub self_concept: String,
    pub warnings: Vec<String>,
}

fn rewrite(reference: &str) -> String {
    match reference.strip_prefix(DRAFT_PROFILE) {
        Some(name) => format!("{PROFILE}{name}"),
        None => reference.to_string(),
    }
}

fn rewrite_keys(map: &mut Json) {
    if let Some(object) = map.as_object_mut() {
        let entries = std::mem::take(object);
        for (key, value) in entries {
            object.insert(rewrite(&key), value);
        }
    }
}

fn number(id: &str) -> u64 {
    id.split_once('-')
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(u64::MAX)
}

fn kind_rank(kind: &str) -> usize {
    [
        "concept",
        "proposition",
        "evidence",
        "assertion",
        "activity",
    ]
    .iter()
    .position(|k| *k == kind)
    .unwrap_or(usize::MAX)
}

/// The draft-typed option Concepts: the draft `Preference` type is gone, and
/// each option is a Concept typed by its kind (Profile `prefers`).
pub fn option_concepts(exported: &Exported) -> Vec<&Json> {
    exported.capsule["payload"]["records"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["schema_ref"].as_str() == Some(&format!("{DRAFT_PROFILE}Preference")))
        .collect()
}

/// Rewrites the exported Capsule onto the KIP 2.0 Profile: symbol references
/// move to `cognitive-memory@2.0.0`, each draft `Preference` becomes the
/// draft-vocabulary type the owner chose for it, and the lineage the draft
/// stored in `derived_from` (now computed, not written) is kept as the
/// `extraction` Activity that produced the Concept.
fn transform(
    exported: &Exported,
    types: &BTreeMap<String, OptionType>,
    packages: &[String],
    report: &mut Report,
) -> Result<Json, BoxError> {
    let mut capsule = exported.capsule.clone();
    let records = capsule["payload"]["records"]
        .as_array_mut()
        .ok_or("the exported Capsule has no records")?;
    let mut missing = Vec::new();
    let mut lineage: Vec<(String, Vec<Json>)> = Vec::new();
    let mut produced: BTreeSet<(String, String)> = BTreeSet::new();
    for record in records.iter_mut() {
        let id = record["id"].as_str().unwrap_or_default().to_string();
        for field in ["schema_ref", "predicate_ref"] {
            if let Some(reference) = record[field].as_str() {
                record[field] = Json::String(rewrite(reference));
            }
        }
        if record["schema_ref"].as_str() == Some(&format!("{PROFILE}Preference")) {
            match types.get(&id) {
                Some(option) => {
                    record["schema_ref"] =
                        Json::String(format!("{DRAFT_VOCABULARY}{}", option.name))
                }
                None => missing.push(id.clone()),
            }
        }
        rewrite_keys(&mut record["structural"]);
        rewrite_keys(&mut record["facets"]);
        if record["kind"] == "concept" {
            record["canonical_id"] = Json::String(canonical(&id));
        }
        if let Some(structural) = record["structural"].as_object_mut()
            && let Some(sources) = structural.remove(&format!("{PROFILE}derived_from"))
        {
            let sources: Vec<Json> = sources.as_array().cloned().unwrap_or_default();
            if !sources.is_empty() {
                lineage.push((id.clone(), sources));
            }
        }
        if record["kind"] == "activity" {
            for output in record["outputs"].as_array().into_iter().flatten() {
                for input in record["inputs"].as_array().into_iter().flatten() {
                    if let (Some(o), Some(i)) = (output["id"].as_str(), input["id"].as_str()) {
                        produced.insert((i.to_string(), o.to_string()));
                    }
                }
            }
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "no option type was chosen for {}; add them to the types file (see --list-options)",
            missing.join(", ")
        )
        .into());
    }

    let mut next_activity = records
        .iter()
        .filter(|r| r["kind"] == "activity")
        .map(|r| number(r["id"].as_str().unwrap_or_default()))
        .max()
        .unwrap_or(0);
    for (concept, sources) in lineage {
        let inputs: Vec<Json> = sources
            .into_iter()
            .filter(|s| {
                s["id"]
                    .as_str()
                    .is_some_and(|i| !produced.contains(&(i.to_string(), concept.clone())))
            })
            .map(|s| json!({"id": s["id"]}))
            .collect();
        if inputs.is_empty() {
            continue;
        }
        next_activity += 1;
        let id = format!("X-{next_activity}");
        report.lineage_activities.push(format!("{id}: {concept}"));
        records.push(json!({
            "id": id,
            "kind": "activity",
            "space_id": DEFAULT_SPACE,
            "activity_class": "extraction",
            "status": "completed",
            "inputs": inputs,
            "outputs": [{"id": concept}],
            "governance": {"authority_class": "descriptive"},
        }));
    }

    // Id order within kind, so the import mints the source's ids.
    records.sort_by(|a, b| {
        let key = |r: &Json| {
            (
                kind_rank(r["kind"].as_str().unwrap_or_default()),
                number(r["id"].as_str().unwrap_or_default()),
            )
        };
        key(a).cmp(&key(b))
    });
    let roots: Vec<Json> = records.iter().map(|r| r["id"].clone()).collect();
    capsule["payload"]["manifest"]["roots"] = Json::Array(roots);
    capsule["payload"]["schema_dependencies"] = Json::Array(
        packages
            .iter()
            .map(|package| json!({"package_ref": package}))
            .collect(),
    );
    Ok(capsule)
}

/// What one transaction may write: its redo plan carries every written row
/// and holds at most 4096 array entries and 16384 JSON nodes.
const CHUNK_ELEMENTS: usize = 500;
const CHUNK_NODES: usize = 10_000;

/// The JSON nodes a value serializes to.
fn nodes(value: &Json) -> usize {
    1 + match value {
        Json::Array(items) => items.iter().map(nodes).sum(),
        Json::Object(map) => map.values().map(nodes).sum(),
        _ => 0,
    }
}

/// The temporary identity a later chunk resolves an already imported Concept
/// by; cleared once every chunk is in.
fn canonical(id: &str) -> String {
    format!("urn:anda-brain:migrate-draft-space:{id}")
}

/// Splits the graph into referentially closed imports. Only Concepts with a
/// temporary canonical id and Propositions can resolve across capsules. Any
/// other mutually connected records (including Assertion evidence and Activity
/// inputs/outputs) must travel together so each source identity is minted once.
fn chunks(capsule: &Json) -> Result<Vec<Vec<Json>>, BoxError> {
    let records = capsule["payload"]["records"]
        .as_array()
        .ok_or("the Capsule has no records")?;
    let by_id: BTreeMap<String, usize> = records
        .iter()
        .enumerate()
        .map(|(index, r)| {
            r["id"]
                .as_str()
                .map(|id| (id.to_string(), index))
                .ok_or("record has no id")
        })
        .collect::<Result<_, _>>()?;
    if by_id.len() != records.len() {
        return Err("the Capsule has duplicate record ids".into());
    }
    fn references(value: &Json, found: &mut BTreeSet<String>) {
        match value {
            Json::String(id) if id.parse::<anda_cognitive_nexus::ElementId>().is_ok() => {
                found.insert(id.clone());
            }
            Json::Array(values) => values.iter().for_each(|v| references(v, found)),
            Json::Object(values) => {
                for (key, value) in values {
                    // Payload and attributes are content, not graph references.
                    if !matches!(
                        key.as_str(),
                        "_system" | "governance" | "payload" | "attributes"
                    ) {
                        references(value, found);
                    }
                }
            }
            _ => {}
        }
    }
    let mut edges = BTreeMap::new();
    for (id, &index) in &by_id {
        let mut refs = BTreeSet::new();
        references(&records[index], &mut refs);
        refs.remove(id);
        for reference in &refs {
            if !by_id.contains_key(reference) {
                return Err(format!("{id} references {reference}, which was not exported").into());
            }
        }
        edges.insert(id.clone(), refs);
    }
    let mut indivisible: BTreeSet<String> = by_id
        .iter()
        .filter(|(_, index)| {
            !matches!(
                records[**index]["kind"].as_str(),
                Some("concept" | "proposition")
            )
        })
        .map(|(id, _)| id.clone())
        .collect();
    // A Concept that refers to a non-reusable record must travel with it too.
    loop {
        let added: Vec<String> = edges
            .iter()
            .filter(|(id, refs)| {
                !indivisible.contains(*id) && refs.iter().any(|r| indivisible.contains(r))
            })
            .map(|(id, _)| id.clone())
            .collect();
        if added.is_empty() {
            break;
        }
        indivisible.extend(added);
    }
    let mut adjacency: BTreeMap<String, BTreeSet<String>> = indivisible
        .iter()
        .map(|id| (id.clone(), BTreeSet::new()))
        .collect();
    for id in &indivisible {
        for reference in &edges[id] {
            if indivisible.contains(reference) {
                adjacency.get_mut(id).unwrap().insert(reference.clone());
                adjacency.get_mut(reference).unwrap().insert(id.clone());
            }
        }
    }
    let mut groups: Vec<BTreeSet<String>> = Vec::new();
    let mut seen = BTreeSet::new();
    // Preserve source order within each kind when assigning components.
    for record in records {
        let id = record["id"].as_str().unwrap();
        if !seen.insert(id.to_string()) {
            continue;
        }
        let mut group = BTreeSet::from([id.to_string()]);
        let mut pending = vec![id.to_string()];
        while let Some(id) = pending.pop() {
            for reference in adjacency.get(&id).into_iter().flatten() {
                if seen.insert(reference.clone()) {
                    group.insert(reference.clone());
                    pending.push(reference.clone());
                }
            }
        }
        groups.push(group);
    }
    let close = |roots: BTreeSet<String>| -> BTreeSet<usize> {
        let mut pending: Vec<String> = roots.iter().cloned().collect();
        let mut included = roots;
        while let Some(id) = pending.pop() {
            for reference in &edges[&id] {
                if included.insert(reference.clone()) {
                    pending.push(reference.clone());
                }
            }
        }
        included.iter().map(|id| by_id[id]).collect()
    };
    // Positions preserve the original kind/id order. Copy a record only when emitting a chunk.
    let materialize = |batch: &BTreeSet<usize>| batch.iter().map(|&i| records[i].clone()).collect();
    let weights: Vec<_> = records.iter().map(nodes).collect();
    let mut out = Vec::new();
    let mut batch = BTreeSet::new();
    let mut weight = 0;
    for group in groups {
        let group = close(group);
        let group_weight = group.iter().map(|&i| weights[i]).sum::<usize>();
        if group.len() > CHUNK_ELEMENTS || group_weight > CHUNK_NODES {
            return Err("a connected reference closure exceeds one migration transaction; the database was not rebuilt".into());
        }
        let mut count = batch.len();
        for i in group.difference(&batch) {
            count += 1;
            weight += weights[*i];
        }
        if count > CHUNK_ELEMENTS || weight > CHUNK_NODES {
            out.push(materialize(&batch));
            batch = group;
            weight = group_weight;
        } else {
            batch.extend(group);
        }
    }
    if !batch.is_empty() {
        out.push(materialize(&batch));
    }
    Ok(out)
}

async fn run(nexus: &CognitiveNexus, text: &str, parameters: Json) -> Result<Json, BoxError> {
    let mut request = Request::single(text);
    if let Json::Object(map) = parameters {
        request.parameters = Some(map);
    }
    let response = nexus
        .system_session()
        .execute(anda_kip::parse_kip(text)?, &request, &request.operations[0])
        .await;
    if response.status != anda_kip::TopLevelStatus::Succeeded {
        if let Some(error) = response
            .error
            .as_ref()
            .or_else(|| response.results.iter().find_map(|r| r.error.as_ref()))
        {
            return Err(format!("{text}: {}: {}", error.code, error.message).into());
        }
        return Err(format!("{text}: {:?}", response.status).into());
    }
    Ok(response.first_result().cloned().unwrap_or(Json::Null))
}

async fn open(root: &Path, space: &str) -> Result<Arc<AndaDB>, BoxError> {
    let os = object_store::local::LocalFileSystem::new_with_prefix(root)?;
    let os = anda_object_store::MetaStoreBuilder::new(os, 100000).build();
    let storage = crate::export::storage_config();
    Ok(Arc::new(
        AndaDB::open(
            Arc::new(os),
            DBConfig {
                name: space.to_string(),
                description: "Anda Brain database".into(),
                storage: anda_db::storage::StorageConfig {
                    cache_max_capacity: storage.cache_max_capacity,
                    cache_max_bytes: storage.cache_max_bytes,
                    compress_level: storage.compress_level,
                    object_chunk_size: storage.object_chunk_size,
                    bucket_overload_size: storage.bucket_overload_size,
                    max_small_object_size: storage.max_small_object_size,
                },
                lock: None,
            },
        )
        .await?,
    ))
}

pub async fn rebuild(
    root: &Path,
    space: &str,
    exported: &Exported,
    types: &BTreeMap<String, OptionType>,
) -> Result<Report, BoxError> {
    let mut report = Report::default();
    if exported.database != space || exported.space_created_at.is_empty() {
        return Err("export does not identify this database; export the target copy into a fresh --work directory".into());
    }
    let option_ids: BTreeSet<_> = option_concepts(exported)
        .into_iter()
        .filter_map(|option| option["id"].as_str())
        .collect();
    let mut defined = BTreeMap::new();
    for (id, option) in types {
        if !option_ids.contains(id.as_str()) {
            return Err(format!("{id} is not a draft Preference in this export").into());
        }
        let symbol: anda_cognitive_nexus::schema::symbol::SymbolRef =
            anda_kip::draft_symbol_ref(&option.name).parse()?;
        if symbol.name != option.name {
            return Err(format!("invalid option type name for {id}").into());
        }
        anda_kip::check_draft_definition(
            anda_kip::DefineKind::ConceptType,
            &json!({"description": option.description}),
        )?;
        defined
            .entry(option.name.clone())
            .or_insert_with(|| option.description.clone());
    }
    if let Some(fact) = exported
        .facts
        .iter()
        .find(|fact| !matches!(fact.state.as_str(), "active" | "archived"))
    {
        return Err(format!("cannot restore {} in storage state {}", fact.id, fact.state).into());
    }
    // The generated legacy package is this Space's own artifact: read it from
    // the old Nexus before that Nexus is dropped.
    let db = open(root, space).await?;
    let old = CognitiveNexus::connect(db.clone()).await?;
    if old.store.spaces().len() != 1 {
        old.close().await?;
        return Err("migration requires a database containing only the default Nexus Space".into());
    }
    let current = old.store.get_space(DEFAULT_SPACE).await?;
    if current.created_at != exported.space_created_at
        || Some(current.seq) != exported.capsule["payload"]["source"]["snapshot_seq"].as_u64()
    {
        old.close().await?;
        return Err(
            "export and target database are from different snapshots; use a fresh --work directory"
                .into(),
        );
    }
    let legacy: Vec<Arc<SchemaPackage>> = old
        .store
        .installed_packages()
        .await?
        .into_iter()
        .filter(|(reference, _)| reference.starts_with("kip://legacy/"))
        .map(|(_, package)| package)
        .collect();
    old.close().await?;

    let profile = SchemaPackage::parse(anda_cognitive_nexus::profiles::COGNITIVE_MEMORY)?;
    let packages: Vec<_> = legacy
        .iter()
        .map(Arc::as_ref)
        .chain([&profile])
        .map(|p| p.manifest.package_ref.clone())
        .collect();
    let mut capsule = transform(exported, types, &packages, &mut report)?;
    // Validate every chunk before dropping any collection. Keep only the common
    // envelope as a template once the records are owned by their import chunks.
    let all = chunks(&capsule)?;
    capsule["payload"]["records"] = json!([]);
    capsule["payload"]["manifest"]["roots"] = json!([]);

    let db = open(root, space).await?;
    for name in db.metadata().collections {
        if NEXUS_COLLECTIONS.contains(&name.as_str()) {
            db.delete_collection(&name).await?;
            report.dropped_collections.push(name);
        } else {
            report.kept_collections.push(name);
        }
    }

    // A clean reopen, so the fresh Nexus sees only what the drop left.
    db.close().await?;
    let db = open(root, space).await?;
    println!(
        "dropped {} Nexus collections; bootstrapping a fresh Nexus",
        report.dropped_collections.len()
    );
    let nexus = CognitiveNexus::connect(db.clone()).await?;
    let mut lock = SchemaLock::default();
    for package in legacy.iter().map(|p| p.as_ref()).chain([&profile]) {
        nexus
            .install_package(package, "migrate-draft-space")
            .await?;
        let manifest = &package.manifest;
        lock.packages
            .insert(manifest.package_id.clone(), manifest.version.clone());
        lock.states
            .insert(manifest.package_id.clone(), PackageState::Active);
        report.packages.push(manifest.package_ref.clone());
    }
    debug_assert!(lock.packages.contains_key(PROFILE_ID));
    nexus.activate_schema(DEFAULT_SPACE, lock).await?;

    // Each option kind is a draft-vocabulary symbol (Spec §20.16).
    for (name, description) in &defined {
        run(
            &nexus,
            "DEFINE CONCEPT TYPE :name {description: :description}",
            json!({"name": name, "description": description}),
        )
        .await?;
        report.defined_types.push(name.clone());
    }

    let symbols: Vec<SymbolMapping> = defined
        .keys()
        .map(|name| SymbolMapping {
            kind: "ConceptType".into(),
            from: format!("{DRAFT_VOCABULARY}{name}"),
            to: name.clone(),
        })
        .collect();
    let mut mapping: BTreeMap<String, String> = BTreeMap::new();
    let total = all.len();
    for (index, chunk) in all.into_iter().enumerate() {
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        for record in &chunk {
            *kinds
                .entry(record["kind"].as_str().unwrap_or_default().to_string())
                .or_default() += 1;
        }
        if (index + 1) % 20 == 0 || index + 1 == total {
            println!(
                "import {}/{total}: last {} ({kinds:?})",
                index + 1,
                chunk
                    .last()
                    .and_then(|r| r["id"].as_str())
                    .unwrap_or_default()
            );
        }
        let mut part = capsule.clone();
        part["payload"]["manifest"]["roots"] =
            Json::Array(chunk.iter().map(|r| r["id"].clone()).collect());
        part["payload"]["records"] = Json::Array(chunk);
        let mut part: anda_kip::Capsule = serde_json::from_value(part)?;
        part.integrity.content_digest =
            anda_cognitive_nexus::capsule::payload_digest(&part.payload)?;
        let imported = anda_cognitive_nexus::capsule::import(
            &nexus,
            &part,
            DEFAULT_SPACE,
            false,
            AuthContext::system(),
            false,
            &symbols,
        )
        .await?;
        for (kind, count) in imported.counts {
            *report.imported.entry(kind).or_default() += count;
        }
        for warning in imported.warnings {
            if !report.warnings.contains(&warning) {
                report.warnings.push(warning);
            }
        }
        mapping.extend(imported.mapping);
    }
    for (source, destination) in &mapping {
        if source != destination {
            report
                .id_changes
                .push((source.clone(), destination.clone()));
        }
    }
    let target =
        |id: &str| -> String { mapping.get(id).cloned().unwrap_or_else(|| id.to_string()) };

    // What an import leaves out. A Space-local key is not an import's to
    // bring (§5.3) nor an UPDATE's to move, so the migration restores the
    // source's own keys directly, and drops the temporary canonical ids the
    // chunked import resolved Concepts by.
    let facts: BTreeMap<&str, &crate::export::Fact> =
        exported.facts.iter().map(|f| (f.id.as_str(), f)).collect();
    let weights: BTreeMap<&str, usize> = exported.capsule["payload"]["records"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["id"].as_str().map(|id| (id, nodes(r))))
        .collect();
    let mut batches: Vec<Vec<&crate::export::Fact>> = Vec::new();
    let mut size = 0;
    for fact in exported.facts.iter().filter(|f| f.kind == "C") {
        let weight = weights.get(fact.id.as_str()).copied().unwrap_or(1);
        match batches.last_mut() {
            Some(batch) if batch.len() < CHUNK_ELEMENTS && size + weight <= CHUNK_NODES => {
                batch.push(fact);
                size += weight;
            }
            _ => {
                batches.push(vec![fact]);
                size = weight;
            }
        }
    }
    for batch in &batches {
        let auth = AuthContext::system();
        let authority = EffectiveAuthority::resolve(&nexus.store, DEFAULT_SPACE, &auth).await?;
        let mut tx = Transaction::begin(
            &nexus.store,
            DEFAULT_SPACE,
            json!({"migration": "cognitive-memory@2.1.0 draft to 2.0.0"}),
            false,
            authority,
            auth,
        )
        .await?;
        for fact in batch {
            let id: anda_cognitive_nexus::ElementId = target(&fact.id).parse()?;
            // Durable records (SleepTask, Watch) change only under a guard.
            let version = tx.load(id).await?.version();
            tx.expect_versions(
                id,
                &[Guard {
                    version,
                    plane: PlaneKey::Element,
                }],
            )
            .await?;
            if let Element::Concept(row) = tx.load(id).await? {
                row.key = fact.key.clone();
                row.canonical_id.clone_from(&fact.canonical_id);
            }
            tx.mark_changed(id, anda_kip::ChangeOp::Update);
            if !fact.key.is_empty() {
                report.keys_restored += 1;
            }
        }
        tx.commit(JournalEntry::default()).await?;
    }
    for fact in facts.values() {
        if fact.state == "archived" {
            match run(
                &nexus,
                "TRANSITION :id TO \"archived\"",
                json!({"id": target(&fact.id)}),
            )
            .await
            {
                Ok(_) => report.archived_restored += 1,
                Err(error) => report
                    .warnings
                    .push(format!("state of {}: {error}", fact.id)),
            }
        }
    }
    if !exported.self_concept.is_empty() {
        let id = target(&exported.self_concept);
        nexus
            .system_session()
            .designate_self(DEFAULT_SPACE, Some(id.parse()?))
            .await?;
        report.self_concept = id;
    }
    nexus.close().await?;
    Ok(report)
}

/// Counts by kind, storage state and type, for the acceptance comparison.
pub struct Census {
    pub counts: BTreeMap<String, usize>,
    pub environment: Json,
    pub packages: Vec<String>,
}

pub async fn census(root: &Path, space: &str) -> Result<Census, BoxError> {
    let db = open(root, space).await?;
    let nexus = CognitiveNexus::connect(db).await?;
    let mut census = BTreeMap::new();
    for (kind, prefix) in [
        (anda_kip::ElementKind::Concept, "C"),
        (anda_kip::ElementKind::Proposition, "P"),
        (anda_kip::ElementKind::Evidence, "E"),
        (anda_kip::ElementKind::Assertion, "A"),
        (anda_kip::ElementKind::Activity, "X"),
    ] {
        for id in nexus.store.elements(kind).ids() {
            let element = nexus
                .store
                .get_element(format!("{prefix}-{id}").parse()?)
                .await?;
            *census
                .entry(format!(
                    "{prefix} {} {}",
                    element.state(),
                    element.schema_ref()
                ))
                .or_default() += 1;
        }
    }
    let environment = run(&nexus, "DESCRIBE SCHEMA ENVIRONMENT", json!({})).await?;
    let packages: Vec<String> = nexus
        .store
        .installed_packages()
        .await?
        .into_keys()
        .collect();
    nexus.close().await?;
    Ok(Census {
        counts: census,
        environment,
        packages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn draft_export_is_bound_to_its_snapshot_and_preserves_canonical_identity() {
        use kip13::Executor as _;
        let dir = tempfile::tempdir().unwrap();
        let name = "migration_roundtrip";
        let os = object_store::local::LocalFileSystem::new_with_prefix(dir.path()).unwrap();
        let os = store13::MetaStoreBuilder::new(os, 100000).build();
        let db = db13::database::AndaDB::connect(
            Arc::new(os),
            db13::database::DBConfig {
                name: name.into(),
                storage: crate::export::storage_config(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let old = nexus13::CognitiveNexus::connect(Arc::new(db))
            .await
            .unwrap();
        old.install_and_activate(
            &[("test", nexus13::profiles::COGNITIVE_MEMORY)],
            DEFAULT_SPACE,
        )
        .await
        .unwrap();
        let command = r#"MUTATE {
            CREATE CONCEPT ?c { TYPE "Person" NAME "Alice" SET FIELDS {canonical_id: "urn:test:alice"} }
            CREATE CONCEPT ?option { TYPE "Preference" NAME "Dark" }
        }"#;
        let request = kip13::Request::single(command);
        let response = old
            .system_session()
            .execute(
                kip13::parse_kip(command).unwrap(),
                &request,
                &request.operations[0],
            )
            .await;
        assert_eq!(
            response.status,
            kip13::TopLevelStatus::Succeeded,
            "{response:?}"
        );
        old.close().await.unwrap();

        let mut exported = crate::export::export(dir.path(), name).await.unwrap();
        let mut types = BTreeMap::from([(
            "C-2".into(),
            OptionType {
                name: String::new(),
                description: "Color scheme".into(),
            },
        )]);
        assert!(rebuild(dir.path(), name, &exported, &types).await.is_err());
        types.get_mut("C-2").unwrap().name = "ColorScheme".into();
        let seq = exported.capsule["payload"]["source"]["snapshot_seq"].take();
        let err = rebuild(dir.path(), name, &exported, &types)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("different snapshots"), "{err}");
        exported.capsule["payload"]["source"]["snapshot_seq"] = seq;
        rebuild(dir.path(), name, &exported, &types).await.unwrap();
        assert_eq!(
            census(dir.path(), name).await.unwrap().counts,
            crate::census_of(&exported, &types)
        );
        let rebuilt = CognitiveNexus::connect(open(dir.path(), name).await.unwrap())
            .await
            .unwrap();
        let Element::Concept(alice) = rebuilt
            .store
            .get_element("C-1".parse().unwrap())
            .await
            .unwrap()
        else {
            panic!("expected Alice")
        };
        assert_eq!(alice.canonical_id, "urn:test:alice");
        rebuilt
            .store
            .open_or_create_space(anda_cognitive_nexus::store::space::SpaceDraft {
                space_id: "another".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        rebuilt.close().await.unwrap();
        let err = rebuild(dir.path(), name, &exported, &types)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("only the default Nexus Space"),
            "{err}"
        );
    }

    #[test]
    fn chunking_keeps_shared_evidence_and_cross_kind_lineage_together() {
        let mut records = vec![
            json!({"kind":"concept","id":"C-1"}),
            json!({"kind":"proposition","id":"P-1","subject":{"id":"C-1"},"object":{"id":"C-1"}}),
        ];
        for id in 1..=300 {
            records.push(json!({"kind":"evidence","id":format!("E-{id}")}));
            records.push(json!({"kind":"assertion","id":format!("A-{id}"),"proposition":{"id":"P-1"},"evidence":[{"id":format!("E-{id}")}]}));
            records.push(json!({"kind":"activity","id":format!("X-{id}"),"inputs":[{"id":format!("E-{id}")}],"outputs":[{"id":format!("A-{id}")}]}));
        }
        let runs = chunks(&json!({"payload":{"records":records}})).unwrap();
        assert!(runs.len() > 1);
        let mut seen = BTreeSet::new();
        for run in runs {
            let ids: BTreeSet<_> = run.iter().map(|r| r["id"].as_str().unwrap()).collect();
            for row in &run {
                let id = row["id"].as_str().unwrap();
                if id.starts_with(['E', 'A', 'X']) {
                    assert!(seen.insert(id.to_string()), "duplicate {id}");
                }
                if row["kind"] == "assertion" {
                    assert!(ids.contains(row["evidence"][0]["id"].as_str().unwrap()));
                }
                if row["kind"] == "activity" {
                    assert!(ids.contains(row["outputs"][0]["id"].as_str().unwrap()));
                }
            }
        }
        assert_eq!(seen.len(), 900);
        assert!(chunks(&json!({"payload":{"records":[{"id":"A-1","kind":"assertion","evidence":[{"id":"E-1"}]}]}})).is_err());
    }

    async fn nexus(name: &str) -> CognitiveNexus {
        let db = AndaDB::connect(
            Arc::new(object_store::memory::InMemory::new()),
            DBConfig {
                name: name.into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let nexus = CognitiveNexus::connect(Arc::new(db)).await.unwrap();
        let profile =
            SchemaPackage::parse(anda_cognitive_nexus::profiles::COGNITIVE_MEMORY).unwrap();
        nexus.install_package(&profile, "test").await.unwrap();
        let mut lock = SchemaLock::default();
        lock.packages.insert(PROFILE_ID.into(), "2.0.0".into());
        lock.states.insert(PROFILE_ID.into(), PackageState::Active);
        nexus.activate_schema(DEFAULT_SPACE, lock).await.unwrap();
        nexus
    }

    #[tokio::test]
    async fn imports_assertion_evidence_and_activity_references_without_duplicates() {
        let source = nexus("migration_source").await;
        run(&source, r#"MUTATE {
            CREATE CONCEPT ?alice { TYPE "Person" NAME "Alice" }
            CREATE CONCEPT ?bob { TYPE "Person" NAME "Bob" }
            ENSURE PROPOSITION ?p (?alice, "same_as", ?bob)
            CREATE EVIDENCE ?e { SET FIELDS {evidence_class: "user_statement", payload: "Same person", observed_at: "2026-01-01T00:00:00.000Z"} }
            CREATE ASSERTION ?a { SET FIELDS {proposition: ?p, asserted_by: ?alice, stance: "support", mode: "stated", confidence: 0.9, asserted_at: "2026-01-01T00:00:00.000Z"} SET STRUCTURAL {("evidence", ?e)} }
            CREATE ACTIVITY ?x { SET FIELDS {activity_class: "extraction", status: "completed"} SET STRUCTURAL {("inputs", ?e) ("outputs", ?a)} }
        }"#, json!({})).await.unwrap();
        let auth = AuthContext::system();
        let authority = EffectiveAuthority::resolve(&source.store, DEFAULT_SPACE, &auth)
            .await
            .unwrap();
        let mut context = anda_cognitive_nexus::kql::Context::open(
            &source.store,
            DEFAULT_SPACE,
            None,
            None,
            &authority,
            &auth,
        )
        .await
        .unwrap();
        let roots = ["C-1", "C-2", "P-1", "E-1", "A-1", "X-1"]
            .into_iter()
            .map(|id| id.parse().unwrap())
            .collect();
        let mut capsule = serde_json::to_value(
            anda_cognitive_nexus::capsule::export(
                &mut context,
                roots,
                &serde_json::Map::from_iter([("closure".into(), json!("selective"))]),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        // Exercise the importer on the owner's complete records, independent
        // of the public exporter's redaction/omission policy.
        let mut records = Vec::new();
        for pattern in [
            "?e CONCEPT {}",
            r#"?e PROPOSITION (id: "P-1")"#,
            "?e EVIDENCE {}",
            "?e ASSERTION {}",
            "?e ACTIVITY {}",
        ] {
            let rows = run(
                &source,
                &format!("FIND(?e) WHERE {{ {pattern} }} LIMIT 20"),
                json!({}),
            )
            .await
            .unwrap();
            records.extend(rows.as_array().unwrap().iter().cloned());
        }
        capsule["payload"]["records"] = json!(records);
        capsule["payload"]["external_refs"] = json!([]);
        let exported = Exported {
            database: "migration_source".into(),
            space_created_at: String::new(),
            capsule,
            facts: vec![],
            self_concept: String::new(),
        };
        let capsule = transform(
            &exported,
            &BTreeMap::new(),
            &[format!("{PROFILE_ID}@2.0.0")],
            &mut Report::default(),
        )
        .unwrap();
        let destination = nexus("migration_destination").await;
        let mut mapping = BTreeMap::new();
        for chunk in chunks(&capsule).unwrap() {
            let mut part = capsule.clone();
            part["payload"]["manifest"]["roots"] =
                json!(chunk.iter().map(|r| r["id"].clone()).collect::<Vec<_>>());
            part["payload"]["records"] = json!(chunk);
            let mut part: anda_kip::Capsule = serde_json::from_value(part).unwrap();
            part.integrity.content_digest =
                anda_cognitive_nexus::capsule::payload_digest(&part.payload).unwrap();
            let report = anda_cognitive_nexus::capsule::import(
                &destination,
                &part,
                DEFAULT_SPACE,
                false,
                AuthContext::system(),
                false,
                &[],
            )
            .await
            .unwrap();
            mapping.extend(report.mapping);
        }
        assert!(
            mapping.iter().all(|(source, target)| source == target),
            "{mapping:?}"
        );
        for (kind, count) in [
            (anda_kip::ElementKind::Concept, 2),
            (anda_kip::ElementKind::Evidence, 1),
            (anda_kip::ElementKind::Assertion, 1),
            (anda_kip::ElementKind::Activity, 1),
        ] {
            assert_eq!(destination.store.elements(kind).ids().len(), count);
        }
        let assertions = run(
            &destination,
            "FIND(?a) WHERE { ?a ASSERTION {} } LIMIT 5",
            json!({}),
        )
        .await
        .unwrap();
        assert_eq!(assertions[0]["evidence"][0]["id"], "E-1");
        destination.close().await.unwrap();
        source.close().await.unwrap();
    }
}
