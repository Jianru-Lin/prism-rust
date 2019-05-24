use crate::experiment::transaction_generator;
use crate::miner::memory_pool::MemoryPool;
use crate::network::server::Handle as ServerHandle;
use crate::wallet::Wallet;
use log::info;
use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use tiny_http::Header;
use tiny_http::Response;
use tiny_http::Server as HTTPServer;
use url::Url;

pub struct Server {
    transaction_generator_handle: mpsc::Sender<transaction_generator::ControlSignal>,
    handle: HTTPServer,
}

#[derive(Serialize)]
struct ApiResponse {
    success: bool,
    message: String,
}

macro_rules! respond {
    ( $req:expr, $success:expr, $message:expr ) => {{
        let content_type = "Content-Type: application/json".parse::<Header>().unwrap();
        let payload = ApiResponse {
            success: $success,
            message: $message.to_string(),
        };
        let resp = Response::from_string(serde_json::to_string_pretty(&payload).unwrap())
            .with_header(content_type);
        $req.respond(resp).unwrap();
    }};
}

impl Server {
    pub fn start(
        addr: std::net::SocketAddr,
        wallet: &Arc<Wallet>,
        server: &ServerHandle,
        mempool: &Arc<Mutex<MemoryPool>>,
        txgen_control_chan: mpsc::Sender<transaction_generator::ControlSignal>,
    ) {
        let handle = HTTPServer::http(&addr).unwrap();
        let server = Self {
            handle: handle,
            transaction_generator_handle: txgen_control_chan,
        };
        thread::spawn(move || {
            for req in server.handle.incoming_requests() {
                let transaction_generator_handle = server.transaction_generator_handle.clone();
                thread::spawn(move || {
                    // a valid url requires a base
                    let base_url = Url::parse(&format!("http://{}/", &addr)).unwrap();
                    let url = match base_url.join(req.url()) {
                        Ok(u) => u,
                        Err(e) => {
                            respond!(req, false, format!("error parsing url: {}", e));
                            return;
                        }
                    };
                    match url.path() {
                        "/transaction-generator/start" => {
                            let control_signal = transaction_generator::ControlSignal::Start;
                            match transaction_generator_handle.send(control_signal) {
                                Ok(()) => respond!(req, true, "ok"),
                                Err(e) => respond!(
                                    req,
                                    false,
                                    format!(
                                        "error sending control signal to transaction generator: {}",
                                        e
                                    )
                                ),
                            }
                        }
                        "/transaction-generator/stop" => {
                            let control_signal = transaction_generator::ControlSignal::Stop;
                            match transaction_generator_handle.send(control_signal) {
                                Ok(()) => respond!(req, true, "ok"),
                                Err(e) => respond!(
                                    req,
                                    false,
                                    format!(
                                        "error sending control signal to transaction generator: {}",
                                        e
                                    )
                                ),
                            }
                        }
                        "/transaction-generator/set-arrival-distribution" => {
                            let params = url.query_pairs();
                            let params: HashMap<_, _> = params.into_owned().collect();
                            let distribution = match params.get("distribution") {
                                Some(v) => v,
                                None => {
                                    respond!(req, false, "missing distribution");
                                    return;
                                }
                            };
                            let distribution = match distribution.as_ref() {
                                "uniform" => {
                                    let interval = match params.get("interval") {
                                        Some(v) => match v.parse::<u64>() {
                                            Ok(v) => v,
                                            Err(e) => {
                                                respond!(
                                                    req,
                                                    false,
                                                    format!("error parsing interval: {}", e)
                                                );
                                                return;
                                            }
                                        },
                                        None => {
                                            respond!(req, false, "missing interval");
                                            return;
                                        }
                                    };
                                    transaction_generator::ArrivalDistribution::Uniform(
                                        transaction_generator::UniformArrival {
                                            interval: interval,
                                        },
                                    )
                                }
                                d => {
                                    respond!(req, false, format!("invalid distribution: {}", d));
                                    return;
                                }
                            };
                            let control_signal =
                                transaction_generator::ControlSignal::SetArrivalDistribution(
                                    distribution,
                                );
                            match transaction_generator_handle.send(control_signal) {
                                Ok(()) => respond!(req, true, "ok"),
                                Err(e) => respond!(
                                    req,
                                    false,
                                    format!(
                                        "error sending control signal to transaction generator: {}",
                                        e
                                    )
                                ),
                            }
                        }
                        "/transaction-generator/set-value-distribution" => {
                            let params = url.query_pairs();
                            let params: HashMap<_, _> = params.into_owned().collect();
                            let distribution = match params.get("distribution") {
                                Some(v) => v,
                                None => {
                                    respond!(req, false, "missing distribution");
                                    return;
                                }
                            };
                            let distribution = match distribution.as_ref() {
                                "uniform" => {
                                    let min = match params.get("min") {
                                        Some(v) => match v.parse::<u64>() {
                                            Ok(v) => v,
                                            Err(e) => {
                                                respond!(
                                                    req,
                                                    false,
                                                    format!("error parsing min: {}", e)
                                                );
                                                return;
                                            }
                                        },
                                        None => {
                                            respond!(req, false, "missing min");
                                            return;
                                        }
                                    };
                                    let max = match params.get("max") {
                                        Some(v) => match v.parse::<u64>() {
                                            Ok(v) => v,
                                            Err(e) => {
                                                respond!(
                                                    req,
                                                    false,
                                                    format!("error parsing max: {}", e)
                                                );
                                                return;
                                            }
                                        },
                                        None => {
                                            respond!(req, false, "missing max");
                                            return;
                                        }
                                    };
                                    if min > max {
                                        respond!(
                                            req,
                                            false,
                                            format!("min value is bigger than max value")
                                        );
                                        return;
                                    }
                                    transaction_generator::ValueDistribution::Uniform(
                                        transaction_generator::UniformValue { min: min, max: max },
                                    )
                                }
                                d => {
                                    respond!(req, false, format!("invalid distribution: {}", d));
                                    return;
                                }
                            };
                            let control_signal =
                                transaction_generator::ControlSignal::SetValueDistribution(
                                    distribution,
                                );
                            match transaction_generator_handle.send(control_signal) {
                                Ok(()) => respond!(req, true, "ok"),
                                Err(e) => respond!(
                                    req,
                                    false,
                                    format!(
                                        "error sending control signal to transaction generator: {}",
                                        e
                                    )
                                ),
                            }
                        }
                        _ => {
                            let content_type =
                                "Content-Type: application/json".parse::<Header>().unwrap();
                            let payload = ApiResponse {
                                success: false,
                                message: "endpoint not found".to_string(),
                            };
                            let resp = Response::from_string(
                                serde_json::to_string_pretty(&payload).unwrap(),
                            )
                            .with_header(content_type)
                            .with_status_code(404);
                            req.respond(resp).unwrap();
                        }
                    }
                });
            }
        });
        info!("API server listening at {}", &addr);
    }
}
