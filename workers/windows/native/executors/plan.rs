//! Prepared execution intent; factories own tool-specific inputs, never the journal.
use crate::executors::parameters::start_parameters;
use crate::{
    artifacts::inputs::{Inputs, Owner},
    executors::resource_process::Request,
    executors::specification::{Specification, default_timeout},
};
use anyhow::Result;
use serde_json::{Map, Value, json};

pub(crate) type Factory = fn(&Map<String, Value>) -> Result<Plan>;
#[derive(Clone)]
pub(crate) enum Response {
    Job,
    Session,
    Command(String),
    Analysis,
}
pub(crate) type Prepare = Box<dyn FnOnce(&Inputs, &Owner) -> Result<Target> + Send>;
pub(crate) enum Target {
    Process(Request),
    Session(crate::executors::session_request::Request),
}
impl Target {
    pub(crate) fn bind(&mut self, resource: String) {
        match self {
            Self::Process(request) => request.resource_id = resource,
            Self::Session(request) => request.target.resource_id = resource,
        }
    }
}
pub(crate) struct Prepared {
    pub(crate) request_id: String,
    pub(crate) specification: Value,
    pub(crate) kind: crate::resources::models::ResourceKind,
    pub(crate) response: Response,
    pub(crate) target: Target,
}
pub(crate) struct Plan {
    pub(crate) kind: crate::resources::models::ResourceKind,

    pub(crate) request_id: String,
    pub(crate) specification: Value,
    pub(crate) prepare: Prepare,
    pub(crate) response: Response,
}
pub(crate) fn process(params: &Map<String, Value>) -> Result<Plan> {
    let (request_id, mut spec): (_, Specification) = start_parameters(params)?;
    if spec.launch_spec.is_none() && spec.timeout_secs.is_none() {
        spec.timeout_secs = Some(default_timeout());
    }
    spec.validate()?;
    Ok(Plan {
        kind: crate::resources::models::ResourceKind::Process,
        request_id: request_id.into(),
        specification: json!({"native_process_schema":crate::executors::resource_process::SCHEMA,"process":spec}),
        response: Response::Job,
        prepare: Box::new(move |inputs, owner| spec.prepare(inputs, owner).map(Target::Process)),
    })
}

impl Plan {
    pub(crate) fn prepare(self, inputs: &Inputs, owner: &Owner) -> Result<Prepared> {
        let target = (self.prepare)(inputs, owner)?;
        Ok(Prepared {
            request_id: self.request_id,
            specification: self.specification,
            kind: self.kind,
            response: self.response,
            target,
        })
    }
}
