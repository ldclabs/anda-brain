//! HTTP binding of the Memory Interface: one endpoint for the five intents,
//! plus the host helpers a business Agent needs around it — source staging,
//! receipt progress and erasure plans. Every handle is readable only by the
//! caller that received it.
use super::*;
use crate::memory_interface::{IncomingRequest, StageSourceInput};
use anda_kip::memory::binding::{Operation, Response as MemoryResponse};

/// A KIP error as an HTTP error, for the helper endpoints (the intent
/// endpoint carries errors inside its Response).
#[allow(clippy::result_large_err)]
fn kip_status(error: anda_kip::KipError) -> AppError {
    use http::StatusCode;
    let status = match error.code {
        anda_kip::KipErrorCode::NotFoundOrNotVisible => StatusCode::NOT_FOUND,
        anda_kip::KipErrorCode::IdempotencyConflict => StatusCode::CONFLICT,
        anda_kip::KipErrorCode::NotAuthorized => StatusCode::FORBIDDEN,
        anda_kip::KipErrorCode::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };
    let message = error.message.clone();
    AppError {
        status,
        message,
        data: serde_json::to_value(anda_kip::ErrorObject::from(error)).ok(),
    }
}

/// POST /v1/{space_id}/memory
///
/// One Memory Interface request in, one Response out (MI §3). A recall needs
/// a read credential and the mutations a write credential; a semantic forget
/// needs an owner (CWT) credential. Failures are `failed` Responses carrying
/// a KIP error, so the HTTP status is 200 once the caller is admitted.
pub async fn post_memory(
    State(app): State<AppState>,
    AppPath(space_id): AppPath<String>,
    Accept(ct, _): Accept,
    HeaderVals(token, sharding): HeaderVals,
    AppBytes(body): AppBytes,
) -> Result<Response, AppError> {
    ensure_sharding(&app, sharding)?;
    let incoming: IncomingRequest = ct
        .parse_body(&body)
        .map_err(AppError::bad_request)?
        .value()
        .map_err(|_| {
            AppError::bad_request("expected a Memory Interface request object".to_string())
        })?;
    let (space, caller) = if incoming.request.operation == Operation::Recall {
        let (space, caller) = read_public(&app, &space_id, &token, sharding, unix_ms()).await?;
        if let Some(reason) = caller.recall_forbidden() {
            return Err(AuthzError::Forbidden(reason).into());
        }
        (space, caller)
    } else {
        credentialed(
            &app,
            &space_id,
            &token,
            sharding,
            TokenScope::Write,
            unix_ms(),
        )
        .await?
    };
    let request = match incoming.resolve() {
        Ok(request) => request,
        Err(response) => return Ok(ct.response(response).into_response()),
    };
    let namespace = caller.namespace();
    // A recall runs a model pass; it shares the LLM request budget.
    let _permit = if request.operation == Operation::Recall {
        Some(
            app.llm_request_semaphore()
                .clone()
                .try_acquire_owned()
                .map_err(|_| {
                    AppError::with_status(
                        http::StatusCode::TOO_MANY_REQUESTS,
                        "too many concurrent requests, retry later".to_string(),
                    )
                })?,
        )
    } else {
        None
    };
    let response: MemoryResponse = space
        .memory_request(&namespace, caller.is_owner(), request)
        .await;
    Ok(ct.response(response).into_response())
}

/// POST /v1/{space_id}/memory/sources
///
/// Stages a captured source and returns its `source_ref` (MI §3).
pub async fn post_memory_source(
    State(app): State<AppState>,
    AppPath(space_id): AppPath<String>,
    Accept(ct, _): Accept,
    HeaderVals(token, sharding): HeaderVals,
    AppBytes(body): AppBytes,
) -> Result<impl IntoResponse, AppError> {
    ensure_sharding(&app, sharding)?;
    let input: StageSourceInput = ct
        .parse_body(&body)
        .map_err(AppError::bad_request)?
        .value()
        .map_err(|_| AppError::bad_request("expected a JSON object body".to_string()))?;
    let (space, caller) = credentialed(
        &app,
        &space_id,
        &token,
        sharding,
        TokenScope::Write,
        unix_ms(),
    )
    .await?;
    let staged = space
        .stage_memory_source(&caller.namespace(), input)
        .await
        .map_err(kip_status)?;
    Ok(ct.response(RpcResponse::success(staged)))
}

/// GET /v1/{space_id}/memory/sources/{source_ref}
pub async fn get_memory_source(
    State(app): State<AppState>,
    AppPath((space_id, source_ref)): AppPath<(String, String)>,
    Accept(ct, _): Accept,
    HeaderVals(token, sharding): HeaderVals,
) -> Result<impl IntoResponse, AppError> {
    let (space, caller) = credentialed(
        &app,
        &space_id,
        &token,
        sharding,
        TokenScope::Read,
        unix_ms(),
    )
    .await?;
    let staged = space
        .staged_memory_source(&caller.namespace(), &source_ref)
        .await
        .map_err(kip_status)?;
    Ok(ct.response(RpcResponse::success(staged)))
}

/// GET /v1/{space_id}/memory/receipts/{receipt_ref}
///
/// A receipt's current progress: a read view, never a rewritten outcome.
pub async fn get_memory_receipt(
    State(app): State<AppState>,
    AppPath((space_id, receipt_ref)): AppPath<(String, String)>,
    Accept(ct, _): Accept,
    HeaderVals(token, sharding): HeaderVals,
) -> Result<impl IntoResponse, AppError> {
    let (space, caller) = credentialed(
        &app,
        &space_id,
        &token,
        sharding,
        TokenScope::Read,
        unix_ms(),
    )
    .await?;
    let view = space
        .memory_receipt_view(&caller.namespace(), &receipt_ref)
        .await
        .map_err(kip_status)?;
    Ok(ct.response(RpcResponse::success(view)))
}

/// GET /v1/{space_id}/memory/plans/{plan_ref}
///
/// The ErasurePlan and the host surfaces a forget covered.
pub async fn get_memory_plan(
    State(app): State<AppState>,
    AppPath((space_id, plan_ref)): AppPath<(String, String)>,
    Accept(ct, _): Accept,
    HeaderVals(token, sharding): HeaderVals,
) -> Result<impl IntoResponse, AppError> {
    let (space, caller) = credentialed(
        &app,
        &space_id,
        &token,
        sharding,
        TokenScope::Read,
        unix_ms(),
    )
    .await?;
    let plan = space
        .memory_plan(&caller.namespace(), &plan_ref)
        .await
        .map_err(kip_status)?;
    Ok(ct.response(RpcResponse::success(plan)))
}
