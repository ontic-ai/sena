//! Platform vision events: screen capture digests, visual context.
//!
//! PRIVACY-CRITICAL: Raw image data is never persisted or transmitted.
//! Only SHA256 digests are stored in events.

use std::fmt;
use std::time::SystemTime;

/// Platform vision event types.
#[derive(Debug, Clone)]
pub enum PlatformVisionEvent {
    /// Screen capture event with image digest.
    ScreenCaptureEvent(ScreenCaptureEvent),
}

/// Screen capture event containing only a digest — never raw pixels.
#[derive(Clone)]
pub struct ScreenCaptureEvent {
    /// When the capture was taken.
    pub timestamp: SystemTime,
    /// SHA256 digest of the captured image — never raw pixels.
    pub image_digest: ImageDigest,
    /// Resolution of the captured image (width, height).
    pub resolution: (u32, u32),
    /// Why this capture was triggered.
    pub capture_reason: CaptureReason,
}

impl fmt::Debug for ScreenCaptureEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenCaptureEvent")
            .field("timestamp", &self.timestamp)
            .field("image_digest", &self.image_digest)
            .field("resolution", &self.resolution)
            .field("capture_reason", &self.capture_reason)
            .finish()
    }
}

/// SHA256 digest of an image.
///
/// This type ensures that raw image data is never stored or transmitted.
/// Only the digest is kept for deduplication and privacy-safe comparison.
#[derive(Clone, PartialEq, Eq)]
pub struct ImageDigest([u8; 32]);

impl ImageDigest {
    /// Create a new ImageDigest from a 32-byte array (SHA256 output).
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Get the raw bytes of the digest.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Get a hex string representation of the digest.
    pub fn as_hex(&self) -> String {
        self.0
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join("")
    }
}

impl fmt::Debug for ImageDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ImageDigest([REDACTED])")
    }
}

/// Reason why a screen capture was triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureReason {
    /// Capture was explicitly requested by the user.
    UserRequested,
    /// Capture was triggered by a significant context switch (app/window change).
    ContextSwitch,
    /// Capture was triggered by a scheduled snapshot interval.
    ScheduledSnapshot,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_digest_new_constructs() {
        let bytes = [0u8; 32];
        let digest = ImageDigest::new(bytes);
        assert_eq!(digest.as_bytes(), &bytes);
    }

    #[test]
    fn image_digest_debug_redacts() {
        let digest = ImageDigest::new([42u8; 32]);
        let debug_str = format!("{:?}", digest);
        assert!(debug_str.contains("REDACTED"));
        assert!(!debug_str.contains("42"));
    }

    #[test]
    fn image_digest_as_hex_produces_correct_format() {
        let bytes = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        let digest = ImageDigest::new(bytes);
        let hex = digest.as_hex();
        assert!(hex.starts_with("0123456789abcdef"));
        assert_eq!(hex.len(), 64); // 32 bytes * 2 hex chars
    }

    #[test]
    fn image_digest_equality_works() {
        let digest1 = ImageDigest::new([1u8; 32]);
        let digest2 = ImageDigest::new([1u8; 32]);
        let digest3 = ImageDigest::new([2u8; 32]);
        assert_eq!(digest1, digest2);
        assert_ne!(digest1, digest3);
    }

    #[test]
    fn screen_capture_event_constructs_and_clones() {
        let digest = ImageDigest::new([7u8; 32]);
        let event = ScreenCaptureEvent {
            timestamp: SystemTime::now(),
            image_digest: digest.clone(),
            resolution: (1920, 1080),
            capture_reason: CaptureReason::UserRequested,
        };
        let cloned = event.clone();
        assert_eq!(cloned.image_digest, event.image_digest);
        assert_eq!(cloned.resolution, event.resolution);
        assert_eq!(cloned.capture_reason, event.capture_reason);
    }

    #[test]
    fn screen_capture_event_debug_does_not_leak_digest() {
        let digest = ImageDigest::new([123u8; 32]);
        let event = ScreenCaptureEvent {
            timestamp: SystemTime::now(),
            image_digest: digest,
            resolution: (1920, 1080),
            capture_reason: CaptureReason::ContextSwitch,
        };
        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("REDACTED"));
        assert!(!debug_str.contains("123"));
    }

    #[test]
    fn capture_reason_variants_are_distinct() {
        assert_ne!(CaptureReason::UserRequested, CaptureReason::ContextSwitch);
        assert_ne!(
            CaptureReason::UserRequested,
            CaptureReason::ScheduledSnapshot
        );
        assert_ne!(
            CaptureReason::ContextSwitch,
            CaptureReason::ScheduledSnapshot
        );
    }

    #[test]
    fn platform_vision_event_constructs_and_clones() {
        let digest = ImageDigest::new([9u8; 32]);
        let capture_event = ScreenCaptureEvent {
            timestamp: SystemTime::now(),
            image_digest: digest,
            resolution: (3840, 2160),
            capture_reason: CaptureReason::ScheduledSnapshot,
        };
        let event = PlatformVisionEvent::ScreenCaptureEvent(capture_event.clone());
        let cloned = event.clone();
        match cloned {
            PlatformVisionEvent::ScreenCaptureEvent(ce) => {
                assert_eq!(ce.resolution, (3840, 2160));
            }
        }
    }

    #[test]
    fn all_types_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<ImageDigest>();
        assert_send::<ScreenCaptureEvent>();
        assert_send::<CaptureReason>();
        assert_send::<PlatformVisionEvent>();
    }
}
