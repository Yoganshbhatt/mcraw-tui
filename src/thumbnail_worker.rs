use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;

use crossbeam_channel::{unbounded, Receiver, Sender};

use crate::decoder::Decoder;
use crate::preview::pipeline::params::PreviewParams;
use crate::thumbnail::{cpu_thumbnail, CachedThumbnail};

use std::time::Instant;

#[derive(Debug)]
pub struct ThumbnailRequest {
    pub path: PathBuf,
    pub timestamp_ns: i64,
    pub params: PreviewParams,
}

#[derive(Debug)]
pub struct ThumbnailResult {
    pub path: PathBuf,
    pub sixel: Option<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub error: Option<String>,
}

impl ThumbnailResult {
    pub fn to_cached(&self) -> Option<CachedThumbnail> {
        self.sixel.as_ref().map(|s| CachedThumbnail {
            sixel: s.clone(),
            width: self.width,
            height: self.height,
            encode_time: Instant::now(),
        })
    }
}

pub struct ThumbnailWorkerPool {
    request_tx: Sender<ThumbnailRequest>,
    pub result_rx: Receiver<ThumbnailResult>,
    _handles: Vec<thread::JoinHandle<()>>,
}

impl ThumbnailWorkerPool {
    pub fn new(num_workers: usize) -> Self {
        let (request_tx, request_rx) = unbounded::<ThumbnailRequest>();
        let (result_tx, result_rx) = unbounded::<ThumbnailResult>();

        let mut handles = Vec::new();
        for _ in 0..num_workers {
            let request_rx = request_rx.clone();
            let result_tx = result_tx.clone();
            handles.push(thread::spawn(move || {
                worker_loop(request_rx, result_tx);
            }));
        }

        Self {
            request_tx,
            result_rx,
            _handles: handles,
        }
    }

    pub fn submit(&self, req: ThumbnailRequest) {
        let _ = self.request_tx.send(req);
    }
}

fn worker_loop(request_rx: Receiver<ThumbnailRequest>, result_tx: Sender<ThumbnailResult>) {
    let mut decoders: HashMap<PathBuf, Option<Decoder>> = HashMap::new();

    while let Ok(req) = request_rx.recv() {
        // Lazy-init the decoder for this file path
        let entry = decoders.entry(req.path.clone()).or_insert_with(|| {
            Decoder::new(&req.path).ok()
        });

        let decoder: &Decoder = match entry.as_ref() {
            Some(d) => d,
            None => {
                let _ = result_tx.send(ThumbnailResult {
                    path: req.path,
                    sixel: None,
                    width: 0,
                    height: 0,
                    error: Some("Failed to open decoder".into()),
                });
                continue;
            }
        };

        let load_result: Result<(Vec<u16>, _), _> = decoder.load_frame(req.timestamp_ns);
        match load_result {
            Ok((bayer, _)) => {
                match cpu_thumbnail(&bayer, &req.params) {
                    Ok((sixel, w, h)) => {
                        let _ = result_tx.send(ThumbnailResult {
                            path: req.path,
                            sixel: Some(sixel),
                            width: w,
                            height: h,
                            error: None,
                        });
                    }
                    Err(e) => {
                        let _ = result_tx.send(ThumbnailResult {
                            path: req.path,
                            sixel: None,
                            width: 0,
                            height: 0,
                            error: Some(format!("CPU thumbnail: {}", e)),
                        });
                    }
                }
            }
            Err(e) => {
                let _ = result_tx.send(ThumbnailResult {
                    path: req.path,
                    sixel: None,
                    width: 0,
                    height: 0,
                    error: Some(format!("Decode: {}", e)),
                });
            }
        }
    }
}
