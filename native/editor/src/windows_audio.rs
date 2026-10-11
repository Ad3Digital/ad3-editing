// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Windows PCM output. Create and drive this blocking sink on a playback worker.
use crate::{Result, audio_output::OutputClock, audio_stream::PcmBlock};
use std::{
    ffi::c_void,
    mem::{ManuallyDrop, size_of},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
type Handle = *mut c_void;
#[repr(C, packed(1))]
struct Format {
    tag: u16,
    channels: u16,
    rate: u32,
    bytes: u32,
    align: u16,
    bits: u16,
    extra: u16,
}
#[repr(C)]
struct Header {
    data: *mut u8,
    length: u32,
    recorded: u32,
    user: usize,
    flags: u32,
    loops: u32,
    next: *mut Header,
    reserved: usize,
}
#[repr(C)]
struct Capabilities {
    manufacturer: u16,
    product: u16,
    version: u32,
    name: [u16; 32],
    formats: u32,
    channels: u16,
    reserved: u16,
    support: u32,
}
#[derive(Clone, Debug)]
pub struct OutputDevice {
    pub id: u32,
    pub name: String,
    pub channels: u16,
    pub supports_stereo_48khz: bool,
}
fn pcm_format() -> Format {
    Format {
        tag: 1,
        channels: 2,
        rate: 48000,
        bytes: 192000,
        align: 4,
        bits: 16,
        extra: 0,
    }
}
pub fn output_devices() -> Result<Vec<OutputDevice>> {
    let count = unsafe { waveOutGetNumDevs() };
    if count > 1024 {
        return Err("Windows reports too many audio devices".into());
    }
    let mut devices = Vec::new();
    for id in 0..count {
        let mut caps: Capabilities = unsafe { std::mem::zeroed() };
        let code =
            unsafe { waveOutGetDevCapsW(id as usize, &mut caps, size_of::<Capabilities>() as u32) };
        if code != 0 {
            return Err(format!("Cannot inspect Windows audio device {id} ({code})"));
        }
        let length = caps
            .name
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(caps.name.len());
        let supported =
            unsafe { waveOutOpen(std::ptr::null_mut(), id, &pcm_format(), 0, 0, 1) } == 0;
        devices.push(OutputDevice {
            id,
            name: String::from_utf16_lossy(&caps.name[..length]),
            channels: caps.channels,
            supports_stereo_48khz: supported,
        });
    }
    Ok(devices)
}
#[link(name = "winmm")]
unsafe extern "system" {
    fn waveOutGetNumDevs() -> u32;
    fn waveOutGetDevCapsW(device: usize, caps: *mut Capabilities, size: u32) -> u32;
    fn waveOutOpen(
        handle: *mut Handle,
        device: u32,
        format: *const Format,
        callback: usize,
        instance: usize,
        flags: u32,
    ) -> u32;
    fn waveOutPrepareHeader(handle: Handle, header: *mut Header, size: u32) -> u32;
    fn waveOutWrite(handle: Handle, header: *mut Header, size: u32) -> u32;
    fn waveOutUnprepareHeader(handle: Handle, header: *mut Header, size: u32) -> u32;
    fn waveOutReset(handle: Handle) -> u32;
    fn waveOutClose(handle: Handle) -> u32;
}
pub struct WindowsOutput {
    handle: Handle,
    clock: OutputClock,
    failed: bool,
    submitted: OutputClock,
    queued: std::collections::VecDeque<Buffer>,
}
struct Buffer {
    bytes: ManuallyDrop<Vec<u8>>,
    header: ManuallyDrop<Box<Header>>,
    end_frame: u64,
}
impl Buffer {
    unsafe fn release(&mut self) {
        unsafe {
            ManuallyDrop::drop(&mut self.header);
            ManuallyDrop::drop(&mut self.bytes);
        }
    }
}
impl WindowsOutput {
    pub fn open() -> Result<Self> {
        Self::open_device(None)
    }
    /// Device indices are local to the current Windows enumeration.
    pub fn open_device(device: Option<u32>) -> Result<Self> {
        if device.is_some_and(|id| id >= unsafe { waveOutGetNumDevs() }) {
            return Err("Windows audio device index is out of range".into());
        }
        let format = pcm_format();
        let mut handle = std::ptr::null_mut();
        // The format is borrowed only for this call; no callbacks are registered.
        let code =
            unsafe { waveOutOpen(&mut handle, device.unwrap_or(u32::MAX), &format, 0, 0, 0) };
        if code != 0 {
            return Err(format!("Cannot open Windows audio device ({code})"));
        }
        Ok(Self {
            handle,
            clock: OutputClock::default(),
            failed: false,
            submitted: OutputClock::default(),
            queued: Default::default(),
        })
    }
    pub fn played_frames(&self) -> u64 {
        self.clock.next_frame()
    }
    pub fn write(&mut self, packet: &PcmBlock, cancel: Option<&AtomicBool>) -> Result<()> {
        self.enqueue(packet, cancel)?;
        while !self.queued.is_empty() {
            self.wait_one(cancel)?;
        }
        Ok(())
    }
    pub fn queued_blocks(&self) -> usize {
        self.queued.len()
    }
    pub fn enqueue(&mut self, packet: &PcmBlock, cancel: Option<&AtomicBool>) -> Result<()> {
        if self.failed {
            return Err("Windows audio output must be reopened".into());
        }
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err("Audio output cancelled".into());
        }
        let mut next_clock = OutputClock::default();
        // Validate without advancing the confirmed device clock on I/O failure.
        next_clock.set_position(self.submitted.next_frame());
        if self.queued.len() >= 3 {
            return Err("Windows audio queue is full".into());
        }
        let mut bytes = ManuallyDrop::new(next_clock.encode(packet)?);
        let mut header = ManuallyDrop::new(Box::new(Header {
            data: bytes.as_mut_ptr(),
            length: bytes.len() as u32,
            recorded: 0,
            user: 0,
            flags: 0,
            loops: 0,
            next: std::ptr::null_mut(),
            reserved: 0,
        }));
        let ptr: *mut Header = &mut **header;
        let size = size_of::<Header>() as u32;
        // Both heap allocations stay fixed until unprepare confirms release.
        let prepared = unsafe { waveOutPrepareHeader(self.handle, ptr, size) };
        if prepared != 0 {
            unsafe {
                ManuallyDrop::drop(&mut header);
                ManuallyDrop::drop(&mut bytes);
            }
            self.failed = true;
            return Err(format!("Cannot prepare Windows audio buffer ({prepared})"));
        }
        let written = unsafe { waveOutWrite(self.handle, ptr, size) };
        self.queued.push_back(Buffer {
            bytes,
            header,
            end_frame: next_clock.next_frame(),
        });
        if written != 0 {
            self.failed = true;
            return Err(format!("Cannot submit Windows audio buffer ({written})"));
        }
        self.submitted = next_clock;
        Ok(())
    }
    pub fn wait_one(&mut self, cancel: Option<&AtomicBool>) -> Result<()> {
        if self.failed {
            return Err("Windows audio output must be reopened".into());
        }
        let Some(buffer) = self.queued.front_mut() else {
            return Ok(());
        };
        let ptr: *mut Header = &mut **buffer.header;
        let size = size_of::<Header>() as u32;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut failure = None;
        loop {
            if failure.is_none()
                && (Instant::now() >= deadline
                    || cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)))
            {
                failure = Some("Audio output cancelled or timed out".into());
            }
            if failure.is_some() {
                unsafe {
                    waveOutReset(self.handle);
                }
            }
            let code = unsafe { waveOutUnprepareHeader(self.handle, ptr, size) };
            if code == 0 {
                unsafe {
                    buffer.release();
                }
                break;
            }
            if code != 33 || Instant::now() >= deadline + Duration::from_secs(1) {
                // A failing driver may retain pointers. Leak these bounded
                // allocations instead of freeing memory still owned by Windows.
                self.failed = true;
                return Err(format!("Cannot release Windows audio buffer ({code})"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let buffer = self.queued.pop_front().unwrap();
        if let Some(error) = failure {
            self.failed = true;
            return Err(error);
        }
        self.clock.set_position(buffer.end_frame);
        Ok(())
    }
}
impl Drop for WindowsOutput {
    fn drop(&mut self) {
        unsafe {
            waveOutReset(self.handle);
            for buffer in &mut self.queued {
                if waveOutUnprepareHeader(
                    self.handle,
                    &mut **buffer.header,
                    size_of::<Header>() as u32,
                ) == 0
                {
                    buffer.release();
                }
            }
            waveOutClose(self.handle);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a Windows audio output device"]
    fn enumerated_device_accepts_explicit_selection() {
        assert_eq!(size_of::<Capabilities>(), 84);
        let devices = output_devices().unwrap();
        let device = devices
            .iter()
            .find(|device| device.supports_stereo_48khz)
            .unwrap();
        assert!(!device.name.is_empty());
        let mut output = WindowsOutput::open_device(Some(device.id)).unwrap();
        output
            .write(
                &PcmBlock {
                    first_frame: 0,
                    samples: vec![0.0; 14],
                },
                None,
            )
            .unwrap();
        assert_eq!(output.played_frames(), 7);
        assert!(WindowsOutput::open_device(Some(u32::MAX)).is_err());
    }
    #[test]
    #[ignore = "requires a Windows audio output device"]
    fn device_accepts_silence_and_releases_buffers() {
        let mut output = WindowsOutput::open().unwrap();
        for first_frame in [0, 1024, 2048] {
            output
                .write(
                    &PcmBlock {
                        first_frame,
                        samples: vec![0.0; 2048],
                    },
                    None,
                )
                .unwrap();
        }
        assert_eq!(output.played_frames(), 3072);
        assert!(
            output
                .write(
                    &PcmBlock {
                        first_frame: 3072,
                        samples: vec![0.0; 2]
                    },
                    Some(&AtomicBool::new(true))
                )
                .is_err()
        );
        assert_eq!(output.played_frames(), 3072);
    }
    #[test]
    #[ignore = "requires a Windows audio output device"]
    fn queued_packets_are_bounded_and_only_completed_frames_are_confirmed() {
        let mut output = WindowsOutput::open().unwrap();
        for first_frame in [0, 1024, 2048] {
            output
                .enqueue(
                    &PcmBlock {
                        first_frame,
                        samples: vec![0.0; 2048],
                    },
                    None,
                )
                .unwrap();
        }
        assert_eq!(output.queued_blocks(), 3);
        assert_eq!(output.played_frames(), 0);
        assert!(
            output
                .enqueue(
                    &PcmBlock {
                        first_frame: 3072,
                        samples: vec![0.0; 2]
                    },
                    None
                )
                .is_err()
        );
        output.wait_one(None).unwrap();
        assert_eq!(output.played_frames(), 1024);
        output
            .enqueue(
                &PcmBlock {
                    first_frame: 3072,
                    samples: vec![0.0; 14],
                },
                None,
            )
            .unwrap();
        while output.queued_blocks() > 0 {
            output.wait_one(None).unwrap();
        }
        assert_eq!(output.played_frames(), 3079);
    }
}
