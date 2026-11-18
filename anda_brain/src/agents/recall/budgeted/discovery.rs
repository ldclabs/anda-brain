//! Bounded native discovery for a packet, including when no model pass fits.
use super::*;
use futures::{StreamExt, stream};
use tokio::time::{Instant, timeout_at};

/// Question-matched Concepts the host reads before any model pass.
const DISCOVERY_HITS: usize = 8;
/// What memory holds about one discovered Concept: the claims it takes part in, in
/// either direction, newest first, each with its final belief. A Concept record only
/// names an entity; the facts about it are Propositions and Assertions, which a
/// concept search never returns and which a pass that ran out of planning budget
/// never gets to read.
const CLAIMS: &str = r#"FIND(?p.id, ?p.subject, ?subject.name, ?predicate, ?p.object, ?object.name,
     ?belief.status, ?belief.explanation.excluded,
     ?a.id, ?a.stance, ?a.mode, ?a.asserted_at, ?a.asserted_by, ?actor.name)
WHERE {
  ?p (:entity, ?predicate, ?object)
  UNION { ?p (?subject, ?predicate, :entity) }
  ?a ASSERTION {proposition: ?p, asserted_by: ?actor}
  ?belief BELIEF (?p)
}
WITH EPISTEMIC {purpose: "answer_user", risk: "low", policy: "kip:memory-default"}
ORDER BY ?a.asserted_at DESC
LIMIT 6"#;
// Four independent reads may wait on storage together; output stays in search order.
const CONCURRENT_READS: usize = 4;
const RELATED_CONCEPTS: usize = 4;

impl RecallAgent {
    pub(super) async fn discover_query(
        &self,
        query: &str,
        counterparty: Option<&Json>,
        material: &mut Material,
        started_at: u64,
    ) -> Result<Vec<String>, BoxError> {
        // All search/claim reads share one read window, including waits on the product gate.
        let deadline = Instant::now()
            + READONLY_KIP_TIMEOUT.min(recall_time_remaining(started_at).unwrap_or_default());
        let search = timeout_at(
            deadline,
            self.budget_kip(kip::request_with(
                "SEARCH CONCEPT :query LIMIT 16",
                kip::param("query", query),
            )),
        )
        .await;
        let hits = search
            .as_ref()
            .ok()
            .and_then(|r| r.as_ref().ok())
            .and_then(kip::ok_result)
            .and_then(|result| result["hits"].as_array());
        material.mark(Channel::Kip, true);
        let mut searched = BTreeSet::new();
        let mut roots = Vec::new();
        if let Some(hits) = hits {
            for hit in hits {
                let element = &hit["element"];
                if let Some(id) = element["id"].as_str()
                    && !is_sleep_task(element)
                    && searched.insert(id.to_owned())
                {
                    roots.push(element);
                    if roots.len() == DISCOVERY_HITS {
                        break;
                    }
                }
            }
        } else {
            material.omit(Channel::Kip);
            material.add(
                Channel::Kip,
                Priority::Warning,
                json!({"query_search":"unavailable","meaning":"not evidence of absence"}),
            )?;
        }
        if let Some(person) = counterparty
            && let Some(id) = person["id"].as_str()
            && searched.insert(id.to_owned())
        {
            roots.push(person);
        }
        let pages = self.claim_pages(&roots, deadline).await;
        let mut neighbors = Vec::new();
        // Expand only the endpoints of an accepted relationship, once. Attribution,
        // an excluded assertion or a disputed edge is not a positive relationship.
        for claim in pages.iter().flatten().flatten().filter(|c| {
            c["belief"] == "accepted" && c["stance"] == "support" && c.get("excluded").is_none()
        }) {
            for endpoint in [&claim["subject"], &claim["object"]] {
                if neighbors.len() < RELATED_CONCEPTS
                    && let Some(id) = endpoint["id"].as_str()
                    && id
                        .parse::<anda_cognitive_nexus::ElementId>()
                        .is_ok_and(|e| e.kind == anda_kip::ElementKind::Concept)
                    && searched.insert(id.to_owned())
                {
                    neighbors.push(endpoint.clone());
                }
            }
            if neighbors.len() == RELATED_CONCEPTS {
                break;
            }
        }
        let mut discovered = Vec::new();
        let mut seen_claims = BTreeSet::new();
        for (element, page) in roots.into_iter().zip(pages) {
            discover_skills(element, &mut material.skills);
            admit_claims(material, page, &mut seen_claims, &mut discovered)?;
            if let Some(existing) = material
                .items
                .iter()
                .find(|i| i.content["id"] == element["id"])
            {
                discovered.push(existing.id.clone());
                continue;
            }
            let priority = if contains_program(element) {
                Priority::UnprovenProcedure
            } else {
                Priority::Relevant
            };
            if let Some(id) = material.add(Channel::Kip, priority, element.clone())? {
                discovered.push(id);
            }
        }
        let related: Vec<_> = neighbors.iter().collect();
        for page in self.claim_pages(&related, deadline).await {
            admit_claims(material, page, &mut seen_claims, &mut discovered)?;
        }
        Ok(discovered)
    }

    async fn claim_pages(&self, entities: &[&Json], deadline: Instant) -> Vec<Option<Vec<Json>>> {
        let reads: Vec<_> = entities
            .iter()
            .map(|entity| self.claim_page(entity, deadline))
            .collect();
        stream::iter(reads)
            .buffered(CONCURRENT_READS)
            .collect()
            .await
    }

    async fn claim_page(&self, entity: &Json, deadline: Instant) -> Option<Vec<Json>> {
        if Instant::now() >= deadline {
            return None;
        }
        let request = kip::request_with(CLAIMS, kip::param("entity", json!({"id":entity["id"]})));
        let response = timeout_at(deadline, self.budget_kip(request))
            .await
            .ok()?
            .ok()?;
        Some(
            kip::ok_result(&response)?
                .as_array()?
                .iter()
                .filter_map(|row| claim_item(row, entity))
                .collect(),
        )
    }
}

fn admit_claims(
    material: &mut Material,
    page: Option<Vec<Json>>,
    seen: &mut BTreeSet<String>,
    discovered: &mut Vec<String>,
) -> Result<(), BoxError> {
    let Some(page) = page else {
        material.omit(Channel::Kip);
        return Ok(());
    };
    for claim in page {
        if let Some(assertion) = claim["assertion"].as_str()
            && seen.insert(assertion.to_owned())
            && let Some(id) = material.add(Channel::Kip, Priority::Relevant, claim)?
        {
            discovered.push(id);
        }
    }
    Ok(())
}

fn is_sleep_task(element: &Json) -> bool {
    element["schema_ref"]
        .as_str()
        .is_some_and(|schema| schema.ends_with("/SleepTask"))
}

/// One `CLAIMS` row as a packet item. `entity` is the discovered Concept the row was
/// read for; it stands at whichever endpoint the row left unbound.
fn claim_item(row: &Json, entity: &Json) -> Option<Json> {
    let [
        proposition,
        subject,
        subject_name,
        predicate,
        object,
        object_name,
        belief,
        excluded,
        assertion,
        stance,
        mode,
        asserted_at,
        asserted_by,
        actor_name,
    ] = row.as_array()?.as_slice()
    else {
        return None;
    };
    // A Concept endpoint is delivered with its name; a Literal object is itself.
    let endpoint = |reference: &Json, name: &Json| -> Json {
        let Some(id) = reference.get("id") else {
            return reference.clone();
        };
        let name = if name.is_null() && *id == entity["id"] {
            &entity["name"]
        } else {
            name
        };
        json!({"id": id, "name": name})
    };
    let mut item = json!({
        "kind": "claim",
        "subject": endpoint(subject, subject_name),
        "predicate": predicate,
        "object": endpoint(object, object_name),
        "belief": belief,
        "stance": stance,
        "mode": mode,
        "asserted_at": asserted_at,
        "asserted_by": endpoint(asserted_by, actor_name),
        "proposition": proposition,
        "assertion": assertion,
    });
    // Why this Assertion does not count toward the belief, such as a value that a
    // later one succeeded: the status alone reads as "nothing is known".
    let reasons: Vec<&Json> = excluded
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["assertion_id"] == *assertion)
        .map(|entry| &entry["reason"])
        .collect();
    if !reasons.is_empty() {
        item["excluded"] = json!(reasons);
    }
    Some(item)
}
