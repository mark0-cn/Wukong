//! Admitted resource operations. Routing and wire decoding belong to the caller.
use crate::executors::plan::Plan;
use wukong_worker_protocol::Method;
pub(crate) enum Operation {
    Start(Plan),
    Session(Method),
    Query(Method),
}
