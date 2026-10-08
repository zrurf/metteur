//! Durable reservations shared by every oversight caller, independent of checkpoints.
use crate::{
    DaemonError, DaemonResult,
    llm::{LlmClient, LlmResponse, billing},
    storage::persistence::{Db, cf},
};
use metteur_shared::{
    config::{Config, oversight::OversightConfig},
    llm::{ContextManager, GenerationParams, Usage},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Caller {
    Supervisor,
    Concierge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Call {
    /// Zero identifies historical accounting; it must not refund the durable ledger.
    #[serde(default)]
    pub accounting_version: u32,
    pub id: Uuid,
    pub caller: Caller,
    pub model: String,
    pub reserved: u64,
    pub charged: u64,
    pub state: String,
    pub usage: Option<Usage>,
    pub cost_micros: Option<u64>,
    pub currency: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ledger {
    pub calls: Vec<Call>,
}
#[derive(Debug, Serialize)]
pub struct Summary {
    pub limit: u64,
    pub charged: u64,
    pub warning: bool,
    pub exhausted: bool,
    pub concierge_available: bool,
    pub calls: Vec<Call>,
}
fn key(run: Uuid) -> Vec<u8> {
    format!("oversight:budget:{run}").into_bytes()
}
pub fn load(db: &Db, run: Uuid) -> DaemonResult<Ledger> {
    db.get(cf::EXECUTION_STATE, &key(run))?
        .map(|v| serde_json::from_slice(&v).map_err(|e| DaemonError::Serialization(e.to_string())))
        .transpose()
        .map(Option::unwrap_or_default)
}
fn save(db: &Db, run: Uuid, ledger: &Ledger) -> DaemonResult<()> {
    db.put_durable(
        cf::EXECUTION_STATE,
        &key(run),
        &serde_json::to_vec(ledger).map_err(|e| DaemonError::Serialization(e.to_string()))?,
    )
}
pub fn summary(db: &Db, run: Uuid, config: &OversightConfig) -> DaemonResult<Summary> {
    let ledger = load(db, run)?;
    let charged = ledger.calls.iter().fold(0u64, |sum, c| sum.saturating_add(c.charged));
    Ok(Summary {
        limit: config.run_token_budget,
        charged,
        warning: charged as f64 >= config.run_token_budget as f64 * config.budget_warn_ratio,
        exhausted: charged >= config.run_token_budget,
        concierge_available: config.concierge_model.as_ref().is_some_and(|s| !s.trim().is_empty()),
        calls: ledger.calls,
    })
}
pub fn reserve(
    db: &Db,
    run: Uuid,
    caller: Caller,
    model: &str,
    amount: u64,
    config: &OversightConfig,
) -> DaemonResult<Uuid> {
    let _guard = db
        .oversight_gate
        .lock()
        .map_err(|_| DaemonError::Persistence("oversight lock poisoned".into()))?;
    if super::requests::load(db, run)?.closed {
        return Err(DaemonError::Execution("run is no longer accepting oversight calls".into()));
    }
    let mut ledger = load(db, run)?;
    let used = ledger.calls.iter().fold(0u64, |sum, c| sum.saturating_add(c.charged));
    if amount == 0
        || config.run_token_budget == 0
        || amount > config.run_token_budget.saturating_sub(used)
    {
        return Err(DaemonError::Execution("oversight token budget exhausted".into()));
    }
    let id = Uuid::new_v4();
    ledger.calls.push(Call {
        accounting_version: 1,
        id,
        caller,
        model: model.into(),
        reserved: amount,
        charged: amount,
        state: "in_flight_unknown".into(),
        usage: None,
        cost_micros: None,
        currency: String::new(),
    });
    save(db, run, &ledger)?;
    Ok(id)
}
pub fn settle(
    db: &Db,
    run: Uuid,
    id: Uuid,
    usage: Option<Usage>,
    model_key: &str,
    config: &Config,
) -> DaemonResult<()> {
    let _guard = db
        .oversight_gate
        .lock()
        .map_err(|_| DaemonError::Persistence("oversight lock poisoned".into()))?;
    let mut ledger = load(db, run)?;
    let call = ledger
        .calls
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| DaemonError::NotFound("oversight reservation".into()))?;
    if call.state != "in_flight_unknown" {
        return Err(DaemonError::Execution("reservation already settled".into()));
    }
    call.state = "usage_unknown".into();
    if let Some(u) = usage {
        if u.tokens_reported {
            call.charged =
                u.input_tokens.saturating_add(u.output_tokens);
            call.accounting_version = 1;
            call.state = "reported".into();
            let pricing = config.llm.models.get(model_key).and_then(|m| {
                billing::effective_pricing_at(m, u.input_tokens, &config.billing.timezone, chrono::Utc::now())
            });
            if let Some(cost) = billing::cost(pricing.as_ref(), &config.billing.currency, &u) {
                call.cost_micros = Some(cost.micros);
                call.currency = cost.currency;
            }
        }
        call.usage = Some(u);
    }
    save(db, run, &ledger)
}

/// One bounded provider attempt. Retries must re-enter this function and reserve
/// separately; there is no fallback model or tool execution path.
pub async fn complete(
    db: &Db,
    run: Uuid,
    caller: Caller,
    model_key: &str,
    config: &Config,
    client: &dyn LlmClient,
    context: &ContextManager,
) -> DaemonResult<LlmResponse> {
    let settings = OversightConfig::from_config(config)
        .map_err(|e| DaemonError::Serialization(e.to_string()))?;
    let input =
        context.build().iter().map(metteur_shared::llm::token::estimate_message).sum::<u64>();
    let id = reserve(
        db,
        run,
        caller,
        client.model(),
        input.saturating_add(u64::from(settings.max_output_tokens)),
        &settings,
    )?;
    let timeout = match caller {
        Caller::Concierge => settings.concierge_timeout_ms,
        Caller::Supervisor => settings.review_timeout_ms,
    };
    let params = GenerationParams {
        temperature: Some(settings.temperature),
        max_tokens: Some(settings.max_output_tokens),
        ..Default::default()
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(timeout),
        client.complete(context, &params, &[]),
    )
    .await
    .unwrap_or_else(|_| {
        Err(DaemonError::Llm("oversight request timed out; remote usage may continue".into()))
    });
    settle(db, run, id, result.as_ref().ok().map(|r| r.usage), model_key, config)?;
    result
}
