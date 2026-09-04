//! Helpers shared by the integration tests.

#![allow(
    dead_code,
    clippy::redundant_pub_crate,
    reason = "not every test binary uses every helper; pub(crate) satisfies unreachable_pub"
)]

/// Decodes a hex string, panicking on malformed input so a fixture typo
/// fails the test instead of decoding as zeros.
#[allow(clippy::panic, clippy::expect_used, reason = "test helper")]
pub(crate) fn hex(s: &str) -> Vec<u8> {
    assert!(s.len() & 1 == 0, "odd-length hex fixture: {s}");
    (0..s.len())
        .step_by(2)
        .map(|i| {
            let pair = s.get(i..i.saturating_add(2)).unwrap_or_default();
            u8::from_str_radix(pair, 16).unwrap_or_else(|_| panic!("bad hex pair {pair:?} in {s}"))
        })
        .collect()
}

/// An in-memory [`Transport`](fluke_connect_client::transport::Transport).
#[cfg(feature = "std")]
#[allow(
    clippy::unwrap_used,
    clippy::unused_async_trait_impl,
    reason = "test double; async trait methods are implemented synchronously"
)]
pub(crate) mod mock {
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use fluke_connect_client::transport::{BoxStream, Notification, Transport, TransportError};
    use futures_util::StreamExt as _;
    use tokio::sync::{broadcast, watch};
    use tokio_stream::wrappers::BroadcastStream;

    /// One recorded write: characteristic, value, with-response flag.
    pub(crate) type Write = (u128, Vec<u8>, bool);

    /// Scripted transport that records writes and replays notifications.
    ///
    /// Clones share state. The notification stream ends when
    /// [`drop_link`](Self::drop_link) or `disconnect` is called, or when every
    /// clone is dropped.
    #[derive(Debug, Clone)]
    pub(crate) struct MockTransport {
        /// Values returned by `read`, keyed by characteristic.
        pub(crate) values: Arc<Mutex<HashMap<u128, Vec<u8>>>>,
        /// Every write that was performed, in order.
        pub(crate) writes: Arc<Mutex<Vec<Write>>>,
        /// Characteristics that were subscribed to.
        pub(crate) subscriptions: Arc<Mutex<Vec<u128>>>,
        /// Channel used to inject notifications.
        tx: broadcast::Sender<Notification>,
        /// Set to true when the link is gone.
        closed: watch::Sender<bool>,
        /// How many times `disconnect` was called.
        disconnects: Arc<AtomicUsize>,
        /// When set, `subscribe` fails.
        fail_subscribe: Arc<AtomicBool>,
        /// Characteristics the device does not expose; `subscribe` reports
        /// them as not found.
        missing: Arc<Mutex<HashSet<u128>>>,
    }

    impl MockTransport {
        /// A transport with no readable values.
        pub(crate) fn new() -> Self {
            let (tx, _) = broadcast::channel(64);
            let (closed, _) = watch::channel(false);
            Self {
                values: Arc::default(),
                writes: Arc::default(),
                subscriptions: Arc::default(),
                tx,
                closed,
                disconnects: Arc::default(),
                fail_subscribe: Arc::default(),
                missing: Arc::default(),
            }
        }

        /// Removes a characteristic from the device.
        pub(crate) fn without_characteristic(self, characteristic: u128) -> Self {
            self.missing.lock().unwrap().insert(characteristic);
            self
        }

        /// Adds a readable value.
        pub(crate) fn with_value(self, characteristic: u128, value: &[u8]) -> Self {
            self.values
                .lock()
                .unwrap()
                .insert(characteristic, value.to_vec());
            self
        }

        /// Makes `subscribe` fail.
        pub(crate) fn failing_subscribe(self) -> Self {
            self.fail_subscribe.store(true, Ordering::SeqCst);
            self
        }

        /// Injects a notification.
        pub(crate) fn notify(&self, characteristic: u128, value: &[u8]) {
            self.tx
                .send(Notification {
                    characteristic,
                    value: value.to_vec(),
                })
                .unwrap();
        }

        /// Simulates the device going away: every notification stream ends.
        pub(crate) fn drop_link(&self) {
            self.closed.send_replace(true);
        }

        /// How many times `disconnect` was called.
        pub(crate) fn disconnect_count(&self) -> usize {
            self.disconnects.load(Ordering::SeqCst)
        }
    }

    impl Transport for MockTransport {
        async fn read(&self, characteristic: u128) -> Result<Vec<u8>, TransportError> {
            self.values
                .lock()
                .unwrap()
                .get(&characteristic)
                .cloned()
                .ok_or(TransportError::CharacteristicNotFound(characteristic))
        }

        async fn write(
            &self,
            characteristic: u128,
            value: &[u8],
            with_response: bool,
        ) -> Result<(), TransportError> {
            self.writes
                .lock()
                .unwrap()
                .push((characteristic, value.to_vec(), with_response));
            Ok(())
        }

        async fn subscribe(&self, characteristic: u128) -> Result<(), TransportError> {
            if self.missing.lock().unwrap().contains(&characteristic) {
                return Err(TransportError::CharacteristicNotFound(characteristic));
            }
            if self.fail_subscribe.load(Ordering::SeqCst) {
                return Err(TransportError::NotConnected);
            }
            self.subscriptions.lock().unwrap().push(characteristic);
            Ok(())
        }

        async fn notifications(&self) -> Result<BoxStream<'static, Notification>, TransportError> {
            let rx = self.tx.subscribe();
            let mut closed = self.closed.subscribe();
            let link_gone = async move {
                // Either the link was dropped or every sender is gone.
                let _ = closed.wait_for(|gone| *gone).await;
            };
            Ok(BroadcastStream::new(rx)
                .filter_map(|r| async move { r.ok() })
                .take_until(link_gone)
                .boxed())
        }

        async fn disconnect(&self) -> Result<(), TransportError> {
            self.disconnects.fetch_add(1, Ordering::SeqCst);
            self.closed.send_replace(true);
            Ok(())
        }
    }
}
