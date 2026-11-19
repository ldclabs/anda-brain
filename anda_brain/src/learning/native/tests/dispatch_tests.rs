use super::*;

fn dispatch_input(
    plan: &PairedTrialPlan,
    frozen: &FrozenNativePlan,
    request: &NativeRequest,
    receipt: &NativeReceipt,
) -> NativeDispatchInput {
    let NativeOperation::Attempt(input) = &request.operation else {
        panic!("attempt fixture")
    };
    let expires = anda_cognitive_nexus::time::parse(&input.started_at).unwrap()
        + std::time::Duration::from_millis(plan.execution.budget.elapsed_ms);
    NativeDispatchInput {
        plan: plan.clone(),
        frozen: frozen.clone(),
        pair_id: input.decision.pair_id.clone(),
        arm: input.decision.arm.clone(),
        attempt_id: input.attempt_id.clone(),
        attempt_ref: receipt.handles["attempt"].clone(),
        task_ref: receipt.handles["task"].clone(),
        lease_expires_at: anda_cognitive_nexus::time::format(expires),
    }
}

async fn pinned_attempt_request(
    nexus: &CognitiveNexus,
    plan: &PairedTrialPlan,
    basis: &Json,
    pair: &str,
    arm: NativeArm,
) -> NativeRequest {
    let mut request = attempt_request(plan, basis, pair, arm);
    if let NativeOperation::Attempt(input) = &mut request.operation
        && matches!(input.decision.arm, NativeArm::Treatment { .. })
    {
        let revision = nexus
            .store
            .element_at(
                DEFAULT_SPACE,
                plan.candidate_revision.parse().unwrap(),
                basis["snapshot_seq"].as_u64().unwrap(),
            )
            .await
            .unwrap()
            .unwrap();
        input.decision.applied_revision_version = Some(revision.version());
    }
    request
}

async fn current_basis(nexus: &CognitiveNexus) -> Json {
    command(
        nexus,
        r#"FIND(?b) WHERE {?p PROPOSITION (id:"P-1") ?b BELIEF (?p)}"#,
    )
    .await[0]["basis"]
        .clone()
}

/// Operator fixture only: no runtime method raises any learned revision's
/// influence authority. After the explicit elevation, retain a native proof
/// for the revision's exact new version, as required by dependency validity.
pub(crate) async fn approve_revision(
    nexus: &CognitiveNexus,
    revision: &str,
    basis_proposition: &str,
) -> (Json, String) {
    let row = nexus
        .store
        .get_element(revision.parse().unwrap())
        .await
        .unwrap();
    let attributes = anda_cognitive_nexus::view::render(&row)["attributes"].clone();
    let source = command(
        nexus,
        &format!(
            r#"CREATE EVIDENCE ?source {{SET FIELDS {{evidence_class:"document",payload:{}}}}}"#,
            literal(
                &serde_json::to_string(&json!({"fixture_operator_approved_program": attributes}))
                    .unwrap()
            )
        ),
    )
    .await["handles"]["source"]
        .as_str()
        .unwrap()
        .to_string();
    nexus
        .system_session()
        .elevate_authority(DEFAULT_SPACE, source.parse().unwrap(), "executable")
        .await
        .unwrap();
    nexus
        .system_session()
        .elevate_authority(DEFAULT_SPACE, revision.parse().unwrap(), "executable")
        .await
        .unwrap();
    let query = format!(
        "FIND(?b) WHERE {{?p PROPOSITION (id:{}) ?b BELIEF (?p)}}",
        literal(basis_proposition)
    );
    let basis = command(nexus, &query).await[0]["basis"].clone();
    let version = nexus
        .store
        .get_element(source.parse().unwrap())
        .await
        .unwrap()
        .version();
    let dependency = json!({"basis_seq":basis["snapshot_seq"],"policy_basis":basis,
        "groups":[{"role":"all_of","pins":[{"id":source,"version":version}]}]});
    command(nexus, &format!(r#"CREATE ACTIVITY ?validation {{SET FIELDS {{activity_class:"dependency_validation",status:"completed"}} SET FACET "DependencyBasis" {dependency} SET STRUCTURAL {{("inputs",{}) ("outputs",{})}}}}"#,
        literal(&source), literal(revision))).await;
    (command(nexus, &query).await[0]["basis"].clone(), source)
}

async fn trial(
    native: &NativeLearning,
    nexus: &CognitiveNexus,
    plan: &PairedTrialPlan,
    frozen: &FrozenNativePlan,
) -> String {
    let observer = AuthContext::principal(OBSERVER);
    let mut attempts = vec![];
    let mut outcomes = vec![];
    for pair in plan.pairs.keys() {
        let request = pinned_attempt_request(
            nexus,
            plan,
            &current_basis(nexus).await,
            pair,
            NativeArm::Baseline,
        )
        .await;
        let receipt = native.execute(&request, None).await.unwrap();
        let observed = native
            .execute(
                &outcome_request(plan, frozen, pair, NativeArm::Baseline, &receipt),
                Some(&observer),
            )
            .await
            .unwrap();
        attempts.push(receipt.handles["attempt"].clone());
        outcomes.push(observed.handles["outcome"].clone());
    }
    let input = native
        .prepare_trial(
            plan.clone(),
            frozen.clone(),
            current_basis(nexus).await,
            attempts,
            outcomes,
        )
        .await
        .unwrap();
    native
        .execute(
            &intent("dispatch-trial", NativeOperation::OpenTrial(input)),
            None,
        )
        .await
        .unwrap()
        .handles["trial"]
        .clone()
}

#[tokio::test]
async fn native_dispatch_once_then_lookup_and_reconciles_known_outcome() {
    let (db, nexus, native, plan, _, _) = setup().await;
    let frozen = install(&native, &plan).await;
    let pair = plan.pairs.keys().next().unwrap();
    let request = pinned_attempt_request(
        &nexus,
        &plan,
        &current_basis(&nexus).await,
        pair,
        NativeArm::Baseline,
    )
    .await;
    let receipt = native.execute(&request, None).await.unwrap();
    assert!(receipt.handles.contains_key("task"));
    let input = dispatch_input(&plan, &frozen, &request, &receipt);
    let first = native.authorize_dispatch(&input).await.unwrap();
    assert_eq!(first.action, NativeDispatchAction::Dispatch);
    assert_eq!(first.fencing_token, 1);
    assert_eq!(first.native_dispatch_version, 2);
    // Simulate loss of begin ACK/checkpoint. Supports-idempotency is false;
    // another admission can only authorize an external lookup, never resend.
    let next = native.authorize_dispatch(&input).await.unwrap();
    assert_eq!(next.action, NativeDispatchAction::Lookup);
    assert_eq!(next.idempotency_key, first.idempotency_key);
    let outcome = native
        .execute(
            &outcome_request(&plan, &frozen, pair, NativeArm::Baseline, &receipt),
            Some(&AuthContext::principal(OBSERVER)),
        )
        .await
        .unwrap();
    let reconciled = native
        .reconcile_dispatch(
            &input.attempt_id,
            next.native_dispatch_version,
            &outcome.handles["outcome"],
        )
        .await
        .unwrap();
    assert_eq!(reconciled.state, "completed");
    assert!(reconciled.task_completed, "{reconciled:?}");
    let repeated = native
        .reconcile_dispatch(
            &input.attempt_id,
            next.native_dispatch_version,
            &outcome.handles["outcome"],
        )
        .await
        .unwrap();
    assert!(repeated.task_completed);
    db.close().await.unwrap();
}

#[tokio::test]
async fn treatment_without_executable_authority_never_gets_dispatch_action() {
    let (db, nexus, native, plan, _, _) = setup().await;
    let frozen = install(&native, &plan).await;
    let trial_ref = trial(&native, &nexus, &plan, &frozen).await;
    let request = pinned_attempt_request(
        &nexus,
        &plan,
        &current_basis(&nexus).await,
        plan.pairs.keys().next().unwrap(),
        NativeArm::Treatment { trial_ref },
    )
    .await;
    let receipt = native.execute(&request, None).await.unwrap();
    let error = native
        .authorize_dispatch(&dispatch_input(&plan, &frozen, &request, &receipt))
        .await
        .unwrap_err();
    assert_eq!(error.code, KipErrorCode::NotAuthorized, "{error:?}");
    assert!(error.message.contains("executable"), "{error:?}");
    db.close().await.unwrap();
}

#[tokio::test]
async fn exact_operator_authorized_revision_dispatches_without_adoption() {
    let (db, nexus, native, plan, _, skill) = setup().await;
    approve_revision(&nexus, &plan.candidate_revision, "P-1").await;
    let frozen = install(&native, &plan).await;
    let trial_ref = trial(&native, &nexus, &plan, &frozen).await;
    let request = pinned_attempt_request(
        &nexus,
        &plan,
        &current_basis(&nexus).await,
        plan.pairs.keys().next().unwrap(),
        NativeArm::Treatment { trial_ref },
    )
    .await;
    let receipt = native.execute(&request, None).await.unwrap();
    let admission = native
        .authorize_dispatch(&dispatch_input(&plan, &frozen, &request, &receipt))
        .await
        .unwrap();
    assert_eq!(admission.action, NativeDispatchAction::Dispatch);
    assert_eq!(
        anda_cognitive_nexus::view::render(
            &nexus
                .store
                .get_element(skill.parse().unwrap())
                .await
                .unwrap()
        )["attributes"]["status"],
        "proposed"
    );
    db.close().await.unwrap();
}

#[tokio::test]
async fn authority_withdrawal_and_changed_dependencies_block_dispatch() {
    for withdrawal in [true, false] {
        let (db, nexus, native, plan, _, _) = setup().await;
        let (_, source) = approve_revision(&nexus, &plan.candidate_revision, "P-1").await;
        let frozen = install(&native, &plan).await;
        let trial_ref = trial(&native, &nexus, &plan, &frozen).await;
        if withdrawal {
            nexus
                .system_session()
                .elevate_authority(
                    DEFAULT_SPACE,
                    plan.candidate_revision.parse().unwrap(),
                    "descriptive",
                )
                .await
                .unwrap();
        } else {
            nexus
                .system_session()
                .elevate_authority(DEFAULT_SPACE, source.parse().unwrap(), "descriptive")
                .await
                .unwrap();
        }
        // A fresh Decision basis cannot repair withdrawn revision authority or
        // the revision's own stale producing DependencyBasis.
        let request = pinned_attempt_request(
            &nexus,
            &plan,
            &current_basis(&nexus).await,
            plan.pairs.keys().next().unwrap(),
            NativeArm::Treatment { trial_ref },
        )
        .await;
        let receipt = native.execute(&request, None).await.unwrap();
        let error = native
            .authorize_dispatch(&dispatch_input(&plan, &frozen, &request, &receipt))
            .await
            .unwrap_err();
        assert!(
            error.message.contains(if withdrawal {
                "executable"
            } else {
                "dependency validity"
            }),
            "{error:?}"
        );
        db.close().await.unwrap();
    }
}

#[tokio::test]
async fn changing_current_revision_blocks_the_old_frozen_revision() {
    let (db, nexus, native, plan, _, skill) = setup().await;
    approve_revision(&nexus, &plan.candidate_revision, "P-1").await;
    let frozen = install(&native, &plan).await;
    let trial_ref = trial(&native, &nexus, &plan, &frozen).await;
    let skill_version = nexus
        .store
        .get_element(skill.parse().unwrap())
        .await
        .unwrap()
        .version();
    let behavior =
        json!({"task_family":plan.task_family,"procedure":"new independently selected revision"});
    command(&nexus, &format!(r#"MUTATE {{
        CREATE CONCEPT ?new {{TYPE "SkillRevision" SET ATTRIBUTES {{task_family:{},procedure:"new independently selected revision",behavior_digest:{}}} SET STRUCTURAL {{("revision_of",{})}}}}
        UPDATE {} UNSET STRUCTURAL {{("current_revision",{})}} SET STRUCTURAL {{("current_revision",?new)}} EXPECT VERSION {skill_version}
    }}"#, literal(&plan.task_family), literal(&content_digest(&behavior).unwrap()), literal(&skill), literal(&skill), literal(&plan.candidate_revision))).await;
    let request = pinned_attempt_request(
        &nexus,
        &plan,
        &current_basis(&nexus).await,
        plan.pairs.keys().next().unwrap(),
        NativeArm::Treatment { trial_ref },
    )
    .await;
    let receipt = native.execute(&request, None).await.unwrap();
    let error = native
        .authorize_dispatch(&dispatch_input(&plan, &frozen, &request, &receipt))
        .await
        .unwrap_err();
    assert!(error.message.contains("no longer current"), "{error:?}");
    db.close().await.unwrap();
}

#[tokio::test]
async fn expired_lease_requires_new_fence_before_terminal_bookkeeping() {
    let (db, nexus, native, plan, _, _) = setup().await;
    let frozen = install(&native, &plan).await;
    let pair = plan.pairs.keys().next().unwrap();
    let request = pinned_attempt_request(
        &nexus,
        &plan,
        &current_basis(&nexus).await,
        pair,
        NativeArm::Baseline,
    )
    .await;
    let receipt = native.execute(&request, None).await.unwrap();
    let mut input = dispatch_input(&plan, &frozen, &request, &receipt);
    let expiry = anda_cognitive_nexus::time::parse(&anda_cognitive_nexus::time::now()).unwrap()
        + std::time::Duration::from_secs(2);
    input.lease_expires_at = anda_cognitive_nexus::time::format(expiry);
    let admission = native.authorize_dispatch(&input).await.unwrap();
    let now = anda_cognitive_nexus::time::parse(&anda_cognitive_nexus::time::now()).unwrap();
    let remaining = std::time::Duration::try_from(expiry.duration_since(now)).unwrap_or_default();
    tokio::time::sleep(remaining + std::time::Duration::from_millis(5)).await;
    let stale = anda_kip::execute_request(
        nexus.as_ref(),
        &anda_kip::Request::single(format!(
            "UPDATE {} SET ATTRIBUTES {{status:\"completed\"}}",
            literal(&input.task_ref)
        )),
    )
    .await;
    assert_ne!(
        stale.status,
        anda_kip::TopLevelStatus::Succeeded,
        "expired fence cannot complete directly"
    );
    let outcome = native
        .execute(
            &outcome_request(&plan, &frozen, pair, NativeArm::Baseline, &receipt),
            Some(&AuthContext::principal(OBSERVER)),
        )
        .await
        .unwrap();
    let result = native
        .reconcile_dispatch(
            &input.attempt_id,
            admission.native_dispatch_version,
            &outcome.handles["outcome"],
        )
        .await
        .unwrap();
    assert_eq!(result.state, "completed");
    assert!(result.task_completed);
    assert!(result.task_completion_pending.is_none());
    let task = nexus
        .store
        .get_element(input.task_ref.parse().unwrap())
        .await
        .unwrap();
    let task = anda_cognitive_nexus::view::render(&task);
    assert_eq!(task["attributes"]["status"], "completed");
    assert_eq!(
        task["facets"][format!("{PROFILE}LeaseState")]["fencing_token"],
        admission.fencing_token + 1
    );
    assert!(
        native
            .revalidate_dispatch(&input, &admission)
            .await
            .is_err(),
        "completion recovery cannot revive the expired execution permit"
    );
    db.close().await.unwrap();
}

#[tokio::test]
async fn task_binding_and_real_lease_budget_cannot_be_substituted() {
    let (db, nexus, native, plan, _, _) = setup().await;
    let frozen = install(&native, &plan).await;
    let pairs = plan.pairs.keys().collect::<Vec<_>>();
    let request = pinned_attempt_request(
        &nexus,
        &plan,
        &current_basis(&nexus).await,
        pairs[0],
        NativeArm::Baseline,
    )
    .await;
    let receipt = native.execute(&request, None).await.unwrap();
    let other = native
        .execute(
            &pinned_attempt_request(
                &nexus,
                &plan,
                &current_basis(&nexus).await,
                pairs[1],
                NativeArm::Baseline,
            )
            .await,
            None,
        )
        .await
        .unwrap();
    let input = dispatch_input(&plan, &frozen, &request, &receipt);
    let mut wrong = input.clone();
    wrong.task_ref = other.handles["task"].clone();
    assert!(native.authorize_dispatch(&wrong).await.is_err());
    let mut over_budget = input.clone();
    over_budget.lease_expires_at = "2099-01-01T00:00:00.000Z".into();
    assert!(native.authorize_dispatch(&over_budget).await.is_err());
    assert!(
        native
            .nexus
            .system_session()
            .read_control(
                DEFAULT_SPACE,
                &format!("dispatch/{}", input.attempt_id),
                None
            )
            .await
            .unwrap()
            .is_none()
    );
    db.close().await.unwrap();
}

#[tokio::test]
async fn permit_revalidation_is_no_effect_and_blocks_post_checkpoint_authority_changes() {
    let (db, nexus, native, plan, _, _) = setup().await;
    approve_revision(&nexus, &plan.candidate_revision, "P-1").await;
    let frozen = install(&native, &plan).await;
    let trial_ref = trial(&native, &nexus, &plan, &frozen).await;
    let request = pinned_attempt_request(
        &nexus,
        &plan,
        &current_basis(&nexus).await,
        plan.pairs.keys().next().unwrap(),
        NativeArm::Treatment { trial_ref },
    )
    .await;
    let receipt = native.execute(&request, None).await.unwrap();
    let input = dispatch_input(&plan, &frozen, &request, &receipt);
    let authorization = native.authorize_dispatch(&input).await.unwrap();
    let sequence = nexus.store.get_space(DEFAULT_SPACE).await.unwrap().seq;
    native
        .revalidate_dispatch(&input, &authorization)
        .await
        .unwrap();
    assert_eq!(
        nexus.store.get_space(DEFAULT_SPACE).await.unwrap().seq,
        sequence
    );
    nexus
        .system_session()
        .elevate_authority(
            DEFAULT_SPACE,
            plan.candidate_revision.parse().unwrap(),
            "descriptive",
        )
        .await
        .unwrap();
    let sequence = nexus.store.get_space(DEFAULT_SPACE).await.unwrap().seq;
    assert!(
        native
            .revalidate_dispatch(&input, &authorization)
            .await
            .is_err()
    );
    assert_eq!(
        nexus.store.get_space(DEFAULT_SPACE).await.unwrap().seq,
        sequence
    );
    db.close().await.unwrap();
}
