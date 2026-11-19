//! Exercises the rule through real KIP records and the protected verdict
//! transaction. The deterministic outcomes are protocol fixtures, not an
//! empirical claim that a model learned a procedure.

use super::workflow_fixture::{self, Policy, Requirement};
use super::*;
use anda_cognitive_nexus::{
    CognitiveNexus, content_digest,
    governance::{
        AuthContext,
        store::{GrantDraft, PrincipalDraft},
    },
    nexus::DEFAULT_SPACE,
    profiles::{COGNITIVE_MEMORY, COGNITIVE_MEMORY_ID, COGNITIVE_MEMORY_VERSION},
    schema::{PackageState, SchemaLock, SchemaPackage},
    store::Element,
};
use anda_db::database::AndaDB;
use anda_kip::{
    Executor, Json, Request, TopLevelStatus,
    cognitive::{
        EvaluationInput, EvaluationPolicy, EvaluationRule, EvaluationSamples, ObserverControl,
    },
};
use object_store::memory::InMemory;
use serde_json::json;
use std::sync::Arc;

fn verifier_digest(plan: &PairedTrialPlan) -> String {
    plan.execution.observer_configuration_digest().unwrap()
}

const OBSERVER: &str = "kip:principal:independent-verifier";
use crate::PROFILE;

async fn command(executor: &impl Executor, text: &str) -> Json {
    let response = anda_kip::execute_request(executor, &Request::single(text)).await;
    assert_eq!(
        response.status,
        TopLevelStatus::Succeeded,
        "{text}\n{response:?}"
    );
    response.first_result().cloned().unwrap()
}

async fn basis(nexus: &CognitiveNexus) -> Json {
    command(
        nexus,
        "FIND(?b) WHERE { ?p PROPOSITION (id:\"P-1\") ?b BELIEF (?p) }",
    )
    .await[0]["basis"]
        .clone()
}

async fn replay_records(nexus: &CognitiveNexus, refs: &[String], facet: &str) -> Json {
    let mut values = serde_json::Map::new();
    for reference in refs {
        let row = nexus
            .store
            .get_element(reference.parse().unwrap())
            .await
            .unwrap();
        let view = anda_cognitive_nexus::view::render(&row);
        let mut wrapped = json!({"record":view["facets"][format!("{PROFILE}{facet}").as_str()],
            "principal_id":view["_system"]["origin"]["principal_id"]});
        if let Element::Evidence(e) = row {
            wrapped["status"] = json!(e.status);
            wrapped["corrected_by"] = json!(e.corrected_by);
            wrapped["observed_at"] = json!(e.observed_at);
        }
        values.insert(reference.clone(), wrapped);
    }
    Json::Object(values)
}

async fn execute_arm(
    nexus: &CognitiveNexus,
    plan: &PairedTrialPlan,
    trial_ref: Option<&str>,
) -> (Vec<String>, Vec<String>) {
    let mut attempts = Vec::new();
    let mut outcomes = Vec::new();
    for pair in plan.pairs.keys() {
        let read_basis = basis(nexus).await;
        let revisions = if trial_ref.is_some() {
            plan.treatment_revisions()
        } else {
            vec![]
        };
        let decision = json!({"decision":"act","retrieved_refs":revisions,"used_refs":revisions,
            "applied_revisions":revisions,"basis":read_basis});
        let input_edges = revisions
            .iter()
            .map(|r| format!("(\"inputs\",\"{r}\")"))
            .collect::<Vec<_>>()
            .join(" ");
        let decision_ref = command(nexus, &format!(r#"CREATE ACTIVITY ?d {{SET FIELDS {{activity_class:"action_gate",status:"completed"}} SET FACET "DecisionRecord" {decision} SET STRUCTURAL {{{input_edges}}}}}"#)).await["handles"]["d"].as_str().unwrap().to_string();
        let attempt = json!({"attempt_id":format!("{}-{}-{pair}", if plan.execution.review_of.is_some(){"monitor"}else{"acquire"}, if trial_ref.is_some(){"treatment"}else{"baseline"}),
            "decision_ref":decision_ref,"applied_revisions":revisions,"trial_ref":trial_ref,
            "context":plan.attempt_context(pair).unwrap(),"environment_digest":plan.environment_digest,
            "tool_versions":plan.tool_versions,"selection_policy":plan.pin().unwrap(),
            "preconditions_satisfied":"yes","started_at":anda_cognitive_nexus::time::now()});
        let attempt_ref = command(nexus, &format!(r#"CREATE ACTIVITY ?a {{SET FIELDS {{activity_class:"action_attempt",status:"completed"}} SET FACET "AttemptRecord" {attempt} SET STRUCTURAL {{("inputs","{decision_ref}")}}}}"#)).await["handles"]["a"].as_str().unwrap().to_string();
        let requirement = if plan.execution.review_of.is_some() {
            Requirement::Forbidden
        } else {
            Requirement::Required
        };
        assert_eq!(
            plan.pairs[pair].initial_state_digest,
            workflow_fixture::initial_state_digest(requirement)
        );
        let receipt = workflow_fixture::execute(
            requirement,
            if trial_ref.is_some() {
                Policy::PrepareThenCommit
            } else {
                Policy::Commit
            },
            &plan.execution.budget,
        );
        let verified = workflow_fixture::verify(&receipt, &plan.execution.budget);
        let payload = serde_json::to_string(&verified.to_string()).unwrap();
        // Separate authenticated observer, after the attempt has committed.
        let outcome = json!({"task_family":plan.task_family,"attempt_ref":attempt_ref,
            "metric":"success","window":plan.observation_window,"terminal":true,
            "observation_key":format!("result-{attempt_ref}"),"observer_config_digest":verifier_digest(plan),
            "outcome_status":verified["outcome_status"]});
        let observer = nexus.session(AuthContext::principal(OBSERVER));
        let observed_at = anda_cognitive_nexus::time::now();
        let result = command(&observer, &format!(r#"MUTATE {{
            CREATE EVIDENCE ?e {{SET FIELDS {{evidence_class:"outcome",payload:{payload},observed_at:"{observed_at}"}} SET FACET "OutcomeRecord" {outcome}}}
            CREATE ACTIVITY ?o {{SET FIELDS {{activity_class:"outcome_observation",status:"completed"}} SET STRUCTURAL {{("inputs","{decision_ref}") ("inputs","{attempt_ref}") ("outputs",?e)}}}}
        }}"#)).await;
        attempts.push(attempt_ref);
        outcomes.push(result["handles"]["e"].as_str().unwrap().to_string());
    }
    (attempts, outcomes)
}

#[tokio::test]
async fn paired_trial_replays_a_real_protected_adoption_transaction() {
    let db = Arc::new(
        AndaDB::connect(
            Arc::new(InMemory::new()),
            crate::testkit::db_config("paired_lifecycle"),
        )
        .await
        .unwrap(),
    );
    let nexus = CognitiveNexus::connect(db.clone()).await.unwrap();
    nexus
        .install_package(&SchemaPackage::parse(COGNITIVE_MEMORY).unwrap(), "test")
        .await
        .unwrap();
    let mut lock = SchemaLock::default();
    lock.packages
        .insert(COGNITIVE_MEMORY_ID.into(), COGNITIVE_MEMORY_VERSION.into());
    lock.states
        .insert(COGNITIVE_MEMORY_ID.into(), PackageState::Active);
    nexus.activate_schema(DEFAULT_SPACE, lock).await.unwrap();
    command(
        &nexus,
        r#"MUTATE {
        CREATE CONCEPT ?p {TYPE "Person" NAME "Test actor"}
        CREATE CONCEPT ?v {TYPE "Insight" NAME "Test lesson" SET ATTRIBUTES {summary:"Test lesson"}}
        ENSURE PROPOSITION ?fact (?p,"prefers",?v)
    }"#,
    )
    .await;
    let behavior =
        json!({"task_family":"tool_workflow.precondition.v1","procedure":"prepare then commit"});
    let created = command(&nexus, &format!(r#"MUTATE {{
        CREATE CONCEPT ?s {{TYPE "Skill" SET ATTRIBUTES {{skill_class:"workflow",summary:"prepare then commit",status:"proposed"}} SET STRUCTURAL {{("current_revision",?r)}}}}
        CREATE CONCEPT ?r {{TYPE "SkillRevision" SET ATTRIBUTES {{task_family:"tool_workflow.precondition.v1",procedure:"prepare then commit",behavior_digest:"{}"}} SET STRUCTURAL {{("revision_of",?s)}}}}
    }}"#, content_digest(&behavior).unwrap())).await;
    let revision = created["handles"]["r"].as_str().unwrap();
    let skill = created["handles"]["s"].as_str().unwrap();
    let mut acquisition: Option<AdoptionBasis> = None;
    let mut original_evaluation = Json::Null;
    let mut original_retry = String::new();
    let mut allowed_parameters = Vec::new();
    for round in 0..2 {
        let mut plan = super::tests::plan(8);
        plan.candidate_revision = revision.into();
        plan.execution.review_of = acquisition.clone();
        for case in plan.pairs.values_mut() {
            case.initial_state_digest = workflow_fixture::initial_state_digest(if round == 0 {
                Requirement::Required
            } else {
                Requirement::Forbidden
            });
            case.task_digest = content_digest(&json!({"task_family":plan.task_family,"goal":"commit the workflow within its current requirements"})).unwrap();
        }
        plan.execution.cutoff = anda_cognitive_nexus::time::format(
            anda_cognitive_nexus::time::parse(&anda_cognitive_nexus::time::now()).unwrap()
                + std::time::Duration::from_secs(10),
        );
        plan.execution.review_due_at = anda_cognitive_nexus::time::format(
            anda_cognitive_nexus::time::parse(&plan.execution.cutoff).unwrap()
                + std::time::Duration::from_secs(60),
        );
        let host = nexus.system_session();
        if let Some(basis) = &acquisition {
            let t = replay_records(
                &nexus,
                std::slice::from_ref(&basis.trial_ref),
                "TrialRecord",
            )
            .await;
            let e = replay_records(
                &nexus,
                std::slice::from_ref(&basis.evaluation_ref),
                "EvaluationRecord",
            )
            .await;
            basis
                .validate_records(
                    &t[&basis.trial_ref]["record"],
                    &e[&basis.evaluation_ref]["record"],
                )
                .unwrap();
        }
        assert_eq!(
            host.put_artifact(DEFAULT_SPACE, workflow_contract(), vec![])
                .await
                .unwrap(),
            plan.execution.workflow
        );
        // Enrollment exists before any baseline outcome, not only before trial.
        let parameters = host
            .put_artifact(
                DEFAULT_SPACE,
                plan.artifact().unwrap(),
                acquisition
                    .as_ref()
                    .map(AdoptionBasis::source_refs)
                    .unwrap_or_default(),
            )
            .await
            .unwrap();
        let rule = paired_rule_artifact();
        if round == 0 {
            register_paired_rule(&nexus).unwrap();
        }
        let rule_pin = host
            .put_artifact(DEFAULT_SPACE, rule.clone(), vec![])
            .await
            .unwrap();
        nexus
            .governance()
            .ensure_principal(PrincipalDraft {
                principal_id: OBSERVER.into(),
                principal_class: "service".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        nexus
            .governance()
            .create_grant(
                GrantDraft {
                    space_id: DEFAULT_SPACE.into(),
                    grantee_principal: OBSERVER.into(),
                    actions: vec![
                        "discover".into(),
                        "read".into(),
                        "create".into(),
                        "record_outcome".into(),
                        "derive".into(),
                    ],
                    ..Default::default()
                },
                "kip:principal:system",
            )
            .await
            .unwrap();
        let observers = vec![ObserverControl {
            principal_id: OBSERVER.into(),
            configuration_digest: verifier_digest(&plan),
            control_domain: "independent-test-instrument".into(),
        }];
        let observer_digest = content_digest(&json!(observers)).unwrap();
        let policy = host
            .set_evaluation_policy(
                DEFAULT_SPACE,
                round,
                EvaluationPolicy {
                    id: "paired-test".into(),
                    version: (round + 1).to_string(),
                    allowed_rules: vec![rule_pin.content_digest.clone()],
                    allowed_parameters: {
                        allowed_parameters.push(parameters.content_digest.clone());
                        allowed_parameters.clone()
                    },
                    observers,
                    observer_control_digest: observer_digest.clone(),
                    minimum_independent_attempts: 8,
                    allow_same_principal_observer: false,
                    retain_adoption_on_insufficient: false,
                },
            )
            .await
            .unwrap();
        let (baseline_attempts, baseline_outcomes) = execute_arm(&nexus, &plan, None).await;
        let baseline_a = replay_records(&nexus, &baseline_attempts, "AttemptRecord").await;
        let baseline_o = replay_records(&nexus, &baseline_outcomes, "OutcomeRecord").await;
        let read_basis = basis(&nexus).await;
        let mut material = vec![revision.to_string()];
        material.extend(baseline_attempts.clone());
        material.extend(baseline_outcomes.clone());
        let replay = host
            .put_artifact(
                DEFAULT_SPACE,
                json!({"rule":rule,"parameters":plan.artifact().unwrap(),
        "basis":read_basis,"baseline_attempts":baseline_a,"baseline_outcomes":baseline_o}),
                material.clone(),
            )
            .await
            .unwrap();
        let trial = json!({"revision_refs":[revision],"basis":read_basis,"rule":rule_pin,"parameters":parameters,
        "baseline_attempt_refs":baseline_attempts,"baseline_outcome_refs":baseline_outcomes,
        "comparability":plan.comparability(&observer_digest).unwrap(),"quota":8,
        "observation_window":plan.observation_window,"replay_artifact":replay,
        "evaluation_policy":{"id":"paired-test","version":(round+1).to_string(),"content_digest":content_digest(&policy.value).unwrap()}});
        let trial_ref = command(&nexus, &format!(r#"CREATE ACTIVITY ?t {{SET FIELDS {{activity_class:"trial_open",status:"completed"}} SET FACET "TrialRecord" {trial} SET STRUCTURAL {{("inputs","{revision}")}}}}"#)).await["handles"]["t"].as_str().unwrap().to_string();
        material.push(trial_ref.clone());
        let transitions = if round == 0 {
            vec![("proposed", "trialed", 1), ("trialed", "adopted", 2)]
        } else {
            vec![("adopted", "revoked", 3)]
        };
        for (from, to, version) in transitions {
            let (attempt_refs, outcome_refs) = if to == "trialed" {
                (vec![], vec![])
            } else {
                execute_arm(&nexus, &plan, Some(&trial_ref)).await
            };
            let mut all_attempts = baseline_attempts.clone();
            all_attempts.extend(attempt_refs.clone());
            let mut all_outcomes = baseline_outcomes.clone();
            all_outcomes.extend(outcome_refs.clone());
            let attempts = replay_records(&nexus, &all_attempts, "AttemptRecord").await;
            let outcomes = replay_records(&nexus, &all_outcomes, "OutcomeRecord").await;
            let input = EvaluationInput {
                rule: rule.clone(),
                parameters: plan.artifact().unwrap(),
                trial: trial.clone(),
                attempts: attempts.clone(),
                outcomes: outcomes.clone(),
                samples: EvaluationSamples::default(),
                minimum_independent_attempts: 8,
            };
            if to != "trialed" {
                // Host closes the predeclared window once; no significance peeking.
                let deadline = anda_cognitive_nexus::time::parse(&plan.execution.cutoff).unwrap();
                let now =
                    anda_cognitive_nexus::time::parse(&anda_cognitive_nexus::time::now()).unwrap();
                if let Ok(delay) = std::time::Duration::try_from(deadline.duration_since(now)) {
                    tokio::time::sleep(delay).await;
                }
                plan.execution
                    .validate_settlement(&plan.execution.cutoff, &anda_cognitive_nexus::time::now())
                    .unwrap();
            }
            let comparison = PairedRule.evaluate(&input).unwrap();
            assert_eq!(
                comparison["status"],
                if to == "trialed" {
                    "insufficient"
                } else if to == "adopted" {
                    "improved"
                } else {
                    "not_improved"
                }
            );
            let mut sources = material.clone();
            sources.extend(attempt_refs.clone());
            sources.extend(outcome_refs.clone());
            let replay = host
                .put_artifact(
                    DEFAULT_SPACE,
                    json!({"rule":rule,"parameters":plan.artifact().unwrap(),
            "trial_record":trial,"attempts":attempts,"outcomes":outcomes}),
                    sources,
                )
                .await
                .unwrap();
            let evaluation = json!({"trial_ref":trial_ref,"revision_refs":[revision],"from_status":from,"to_status":to,
            "rule_digest":rule_pin.content_digest,"parameters_digest":parameters.content_digest,
            "cutoff":if to == "trialed" {anda_cognitive_nexus::time::now()} else {plan.execution.cutoff.clone()},"attempt_refs":attempt_refs,"outcome_refs":outcome_refs,
            "excluded_samples":[],"missing_attempt_refs":[],"comparison":comparison,"replay_artifact":replay});
            let mutation = format!(
                r#"MUTATE {{
            CREATE ACTIVITY ?v {{SET FIELDS {{activity_class:"lifecycle_verdict",status:"completed"}} SET FACET "EvaluationRecord" {evaluation} SET STRUCTURAL {{("inputs","{revision}") ("inputs","{trial_ref}") ("outputs","{skill}")}}}}
            UPDATE "{skill}" SET ATTRIBUTES {{status:"{to}"}} SET STRUCTURAL {{("current_trial","{trial_ref}") ("current_evaluation",?v)}} EXPECT VERSION {version}
        }}"#
            );
            if to == "adopted" {
                let forged = mutation.replace("\"effect\":1.0", "\"effect\":0.9");
                assert_ne!(forged, mutation);
                let response = anda_kip::execute_request(&nexus, &Request::single(forged)).await;
                assert_eq!(
                    response.status,
                    TopLevelStatus::Failed,
                    "forged verdict must not commit"
                );
            }
            let committed = command(&nexus, &mutation).await;
            if to == "adopted" {
                let evaluation_ref = committed["handles"]["v"].as_str().unwrap().to_string();
                acquisition = Some(AdoptionBasis {
                    revision_ref: revision.into(),
                    trial_ref: trial_ref.clone(),
                    evaluation_ref: evaluation_ref.clone(),
                });
                original_evaluation =
                    replay_records(&nexus, &[evaluation_ref], "EvaluationRecord").await;
                original_retry = mutation
                    .replace("\"from_status\":\"trialed\"", "\"from_status\":\"revoked\"")
                    .replace("\"to_status\":\"adopted\"", "\"to_status\":\"trialed\"")
                    .replace("status:\"adopted\"", "status:\"trialed\"")
                    .replace("EXPECT VERSION 2", "EXPECT VERSION 4");
            }
            let row = nexus
                .store
                .get_element(skill.parse().unwrap())
                .await
                .unwrap();
            assert_eq!(
                anda_cognitive_nexus::view::render(&row)["attributes"]["status"],
                to
            );
        }
    }
    let acquisition = acquisition.unwrap();
    assert_eq!(
        replay_records(&nexus, &[acquisition.evaluation_ref], "EvaluationRecord").await,
        original_evaluation,
        "monitoring retains the acquisition verdict unchanged"
    );
    let old_trial = anda_kip::execute_request(&nexus, &Request::single(original_retry)).await;
    assert_eq!(
        old_trial.status,
        TopLevelStatus::Failed,
        "a revoked revision cannot reenter using its acquisition trial"
    );
    assert!(
        serde_json::to_string(&old_trial)
            .unwrap()
            .contains("re-entry requires a trial opened after revocation"),
        "{old_trial:?}"
    );
    db.close().await.unwrap();
}
