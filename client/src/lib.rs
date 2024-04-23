use std::collections::HashMap;
use std::str::FromStr;

use alloy_primitives::Address as AlloyAddress;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use kinode_process_lib::{timer, vfs};
use kinode_process_lib::{
    await_message, call_init, get_blob, get_typed_state, println, set_state,
    Address, Message, LazyLoadBlob, ProcessId, Request, Response,
};

wit_bindgen::generate!({
    path: "wit",
    world: "process",
});

#[derive(Debug, Serialize, Deserialize)]
struct State {
    current_job: Option<CurrentJob>,
    router_process: Option<ProcessId>,
    rollup_sequencer: Option<Address>,
    on_chain_state: OnChainDaoState,
}

#[derive(Debug, Serialize, Deserialize)]
struct CurrentJob {
    job_id: u64,
    next_image_number: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OnChainDaoState {
    pub routers: Vec<String>,  // length 1 for now
    pub members: HashMap<String, AlloyAddress>,
    pub proposals: HashMap<u64, ProposalInProgress>,
    // pub client_blacklist: Vec<String>,
    // pub member_blacklist: Vec<String>,
    pub queue_response_timeout_seconds: u8,
    pub serve_timeout_seconds: u16, // TODO
    pub max_outstanding_payments: u8,
    pub payment_period_hours: u8,
}

/// Possible proposals
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Proposal {
    ChangeRootNode(String),
    ChangeQueueResponseTimeoutSeconds(u8),
    ChangeMaxOutstandingPayments(u8),
    ChangePaymentPeriodHours(u8),
    Kick(String),
}

/// Possible proposals
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProposalInProgress {
    pub proposal: Proposal,
    pub votes: HashMap<String, SignedVote>,
}

/// A vote on a proposal
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Vote {
    pub proposal_hash: u64,
    pub is_yea: bool,
}

/// A signed vote on a proposal
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SignedVote {
    vote: Vote,
    signature: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            current_job: None,
            router_process: None,
            rollup_sequencer: None,
            on_chain_state: OnChainDaoState::default(),
        }
    }
}

impl Default for OnChainDaoState {
    fn default() -> Self {
        // TODO: get state from rollup
        Self {
            routers: vec![],
            members: HashMap::new(),
            proposals: HashMap::new(),
            queue_response_timeout_seconds: 0,
            serve_timeout_seconds: 0,
            max_outstanding_payments: 0,
            payment_period_hours: 0,
        }
    }
}

impl State {
    fn save(&self) -> anyhow::Result<()> {
        set_state(&serde_json::to_vec(self)?);
        Ok(())
    }

    fn load() -> Self {
        match get_typed_state(|bytes| Ok(serde_json::from_slice::<State>(bytes)?)) {
            Some(rs) => rs,
            None => State::default(),
        }
    }
}

#[derive(Error, Debug)]
enum NotAMatchError {
    #[error("Match failed")]
    NotAMatch
}

#[derive(Debug, Serialize, Deserialize)]
enum PublicRequest {
    RunJob(JobParameters),
    /// Parameters in LazyLoadBlob.
    JobUpdate { job_id: u64, is_final: bool, signature: Result<u64, String> },
}

#[derive(Debug, Serialize, Deserialize)]
enum PublicResponse {
    RunJob(RunResponse),
    JobUpdate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JobParameters {
    pub workflow: String,
    pub parameters: String,
}

#[derive(Debug, Serialize, Deserialize)]
enum RunResponse {
    JobQueued { job_id: u64 },
    PaymentRequired,
    Error(String),
}

#[derive(Debug, Serialize, Deserialize)]
enum AdminRequest {
    SetRouterProcess { process_id: String },
    SetRollupSequencer { address: String },
    GetRollupState,
}

#[derive(Debug, Serialize, Deserialize)]
enum AdminResponse {
    SetRouterProcess { err: Option<String> },
    SetRollupSequencer { err: Option<String> },
    GetRollupState { err: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SequencerRequest {
    Read(ReadRequest),
    //Write(SignedTransaction<OnChainDaoState>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SequencerResponse {
    Read(ReadResponse),
    Write,  // TODO: return hash of tx?
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ReadRequest {
    All,
    Dao,
    Routers,
    Members,
    Proposals,
    Parameters,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ReadResponse {
    All(OnChainDaoState),
    Dao,
    Routers(Vec<String>),  // length 1 for now
    Members(Vec<String>),  // TODO: should probably be the HashMap
    Proposals,
    Parameters,
}

fn await_chain_state(state: &mut State) -> anyhow::Result<()> {
    let Some(rollup_sequencer) = state.rollup_sequencer.clone() else {
        println!("err: {:?}", state);
        return Err(anyhow::anyhow!("fetch_chain_state rollup_sequencer must be set before chain state can be fetched"));
    };
    Request::to(rollup_sequencer)  // TODO
        .body(vec![])
        .blob_bytes(serde_json::to_vec(&SequencerRequest::Read(ReadRequest::All))?)
        .send_and_await_response(5)??;
    let Some(LazyLoadBlob { ref bytes, .. }) = get_blob() else {
        println!("err: no blob");
        return Err(anyhow::anyhow!("fetch_chain_state didn't get back blob"));
    };
    let Ok(SequencerResponse::Read(ReadResponse::All(new_dao_state))) = serde_json::from_slice(bytes) else {
        println!("err: {:?}", serde_json::from_slice::<serde_json::Value>(bytes));
        return Err(anyhow::anyhow!("fetch_chain_state got wrong Response back"));
    };
    state.on_chain_state = new_dao_state.clone();
    state.save()?;
    Ok(())
}

fn handle_public_request(
    our: &Address,
    message: &Message,
    images_dir: &str,
    state: &mut State,
) -> anyhow::Result<()> {
    match serde_json::from_slice(message.body()) {
        Ok(PublicRequest::RunJob(_job_parameters)) => {
            if state.current_job.is_some() {
                return Err(anyhow::anyhow!("wait until current job is done"));
            }
            if state.router_process.is_none() {
                return Err(anyhow::anyhow!("cannot send job until AdminRequest::SetRouterProcess"));
            };
            if state.rollup_sequencer.is_none() {
                return Err(anyhow::anyhow!("cannot send job until AdminRequest::SetRollupSequencer"));
            };

            let address = Address::new(
                state.on_chain_state.routers[0].clone(),
                state.router_process.clone().unwrap(),
            );
            Request::to(address)
                .body(message.body())
                .expects_response(20)
                .send()?;
        }
        Ok(PublicRequest::JobUpdate { job_id, is_final, signature }) => {
            let Some(ref mut current_job) = state.current_job else {
                println!("unexpectedly got JobUpdate with no current_job set");
                state.current_job = Some(CurrentJob {
                    job_id,
                    next_image_number: 0,
                });
                state.save()?;
                return handle_public_request(our, message, images_dir, state);
            };
            let Some(LazyLoadBlob { ref bytes, .. }) = get_blob() else {
                return Err(anyhow::anyhow!("got PublicRequest::JobUpdate with no blob"));
            };
            let file = format!(
                "{images_dir}/{job_id}-{}.jpg",
                if is_final { "final".to_string() } else { current_job.next_image_number.to_string() },
            );
            current_job.next_image_number += 1;
            if is_final {
                // done!
                state.current_job = None;
            }
            state.save()?;
            let file = vfs::open_file(&file, true, None)?;
            file.write(bytes)?;
        }
        Err(_e) => {
            return Err(NotAMatchError::NotAMatch.into());
        }
    }
    Ok(())
}

fn handle_public_response(
    message: &Message,
    state: &mut State,
) -> anyhow::Result<()> {
    match serde_json::from_slice(message.body()) {
        Ok(PublicResponse::RunJob(response)) => {
            match response {
                RunResponse::JobQueued { job_id } => {
                    timer::set_timer(10 * 1000, Some(serde_json::to_vec(&job_id)?)); // TODO
                    state.current_job = Some(CurrentJob {
                        job_id,
                        next_image_number: 0,
                    });
                    state.save()?;
                    println!("get RunResponse::JobQueued for {job_id}");
                }
                RunResponse::PaymentRequired => {
                    println!("got RunResponse::PaymentRequired");
                }
                RunResponse::Error(e) => {
                    println!("got RunResponse::Error: {e}");
                }
            }
        }
        Ok(PublicResponse::JobUpdate) => {}
        Err(_e) => {
            return Err(NotAMatchError::NotAMatch.into());
        }
    }
    Ok(())
}

fn handle_admin_request(
    our: &Address,
    message: &Message,
    state: &mut State,
) -> anyhow::Result<()> {
    let source = message.source();
    if source.node() != our.node() {
        if serde_json::from_slice::<AdminRequest>(message.body()).is_err() {
            return Err(NotAMatchError::NotAMatch.into());
        }
        return Err(anyhow::anyhow!("only our can make AdminRequests; rejecting from {source:?}"));
    }
    match serde_json::from_slice(message.body()) {
        Ok(AdminRequest::SetRouterProcess { process_id }) => {
            let process_id = process_id.parse()?;
            state.router_process = Some(process_id);
            state.save()?;
            Response::new()
                .body(serde_json::to_vec(&AdminResponse::SetRouterProcess { err: None })?)
                .send()?;
        }
        Ok(AdminRequest::SetRollupSequencer { address }) => {
            let address = address.parse()?;
            state.rollup_sequencer = Some(address);
            state.save()?;
            await_chain_state(state)?;
            Response::new()
                .body(serde_json::to_vec(&AdminResponse::SetRollupSequencer { err: None })?)
                .send()?;
        }
        Ok(AdminRequest::GetRollupState) => {
            if state.rollup_sequencer.is_none() {
                let err = "no rollup sequencer set";
                Response::new()
                    .body(serde_json::to_vec(&AdminResponse::GetRollupState {
                        err: Some(err.to_string())
                    })?)
                    .send()?;
                return Err(anyhow::anyhow!(err));
            }
            await_chain_state(state)?;
            Response::new()
                .body(serde_json::to_vec(&AdminResponse::GetRollupState { err: None })?)
                .send()?;
        }
        Err(e) => {
            return Err(NotAMatchError::NotAMatch.into());
        }
    }
    Ok(())
}

fn handle_message(
    our: &Address,
    message: &Message,
    images_dir: &str,
    state: &mut State,
) -> anyhow::Result<()> {
    if message.is_request() {
        match handle_admin_request(our, message, state) {
            Ok(_) => return Ok(()),
            Err(e) => {
                if e.downcast_ref::<NotAMatchError>().is_none() {
                    return Err(e);
                }
            }
        }
        match handle_public_request(our, message, images_dir, state) {
            Ok(_) => return Ok(()),
            Err(e) => {
                if e.downcast_ref::<NotAMatchError>().is_none() {
                    return Err(e);
                }
            }
        }
        return Err(anyhow::anyhow!(
            "unexpected request from {:?}: {:?}",
            message.source(),
            serde_json::from_slice::<serde_json::Value>(message.body()),
        ));
    }
    match handle_public_response(message, state) {
        Ok(_) => return Ok(()),
        Err(e) => {
            if e.downcast_ref::<NotAMatchError>().is_none() {
                return Err(e);
            }
        }
    }
    if message.source().to_string() == format!("{}@timer:distro:sys", our.node()) {
        let Some(ref current_job) = state.current_job else {
            // job already finished
            return Ok(());
        };
        let timer_job_id: u64 = serde_json::from_slice(message.context().unwrap_or_default())?;
        if current_job.job_id == timer_job_id {
            state.current_job = None;
            state.save()?;
            return Err(anyhow::anyhow!("job {} timed out", timer_job_id));
        }
    }
    Ok(())
}

call_init!(init);
fn init(our: Address) {
    println!("{}: begin", our.process());

    let images_dir = vfs::create_drive(our.package_id(), "images", None).unwrap();
    let mut state = State::load();

    loop {
        let message = match await_message() {
            Ok(m) => m,
            Err(_send_err) => {
                println!("SendError");
                state.current_job = None;
                state.save().unwrap();
                continue;
            },
        };
        match handle_message(
            &our,
            &message,
            &images_dir,
            &mut state,
        ) {
            Ok(()) => {}
            Err(e) => {
                println!("{}: error: {:?}", our.process(), e);
            }
        };
    }
}
