use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::Duration;

use cdma_bts::bts::hrpd::power_control::{
    HrpdPacketExclusion, HrpdPacketObservation, HrpdPacketOutcome, HrpdPowerControlRegistry,
};

const WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const UATI: u32 = 0x8005_8001;
const MAC_INDEX: u8 = 5;

struct BlockingLogger {
    entered: mpsc::Sender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}

impl log::Log for BlockingLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target() == "cdma_bts::bts::hrpd::power_control"
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            self.entered.send(()).unwrap();
            self.resume.lock().unwrap().recv().unwrap();
        }
    }

    fn flush(&self) {}
}

#[test]
fn target_reads_continue_while_power_control_logging_is_blocked() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let logger = Box::leak(Box::new(BlockingLogger {
        entered: entered_tx,
        resume: Mutex::new(resume_rx),
    }));
    log::set_logger(logger).unwrap();
    log::set_max_level(log::LevelFilter::Trace);
    let registry = HrpdPowerControlRegistry::default();
    let mut handle = None;

    for operation in ["install", "report", "suspend", "resume", "release"] {
        let writer_registry = registry.clone();
        let writer = thread::spawn(move || {
            if operation == "install" {
                handle = Some(writer_registry.install(UATI, MAC_INDEX));
            } else {
                let active = handle.as_ref().unwrap();
                match operation {
                    "report" => {
                        active.report(HrpdPacketObservation::rev0(HrpdPacketOutcome::Excluded(
                            HrpdPacketExclusion::MobilePowerLimited,
                        )));
                    }
                    "suspend" => assert!(active.suspend_for_tune_away()),
                    "resume" => assert!(active.resume_after_tune_away()),
                    "release" => writer_registry.release(UATI, MAC_INDEX),
                    _ => unreachable!(),
                }
            }
            handle
        });
        entered_rx.recv_timeout(WAIT_TIMEOUT).unwrap();
        let reader_registry = registry.clone();
        let (read_tx, read_rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            read_tx.send(reader_registry.snapshots()).unwrap();
        });
        let snapshot = read_rx.recv_timeout(WAIT_TIMEOUT);
        resume_tx.send(()).unwrap();
        handle = writer.join().unwrap();
        reader.join().unwrap();
        let snapshot = snapshot.unwrap_or_else(|error| {
            panic!("{operation} blocked state reads while logging: {error}")
        });
        assert_eq!(snapshot.len(), usize::from(operation != "release"));
    }
    log::set_max_level(log::LevelFilter::Off);
}
