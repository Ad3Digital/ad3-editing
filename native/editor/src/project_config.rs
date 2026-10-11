// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Editor metadata remains in the existing package.json diffusion field.
use crate::{Result, atomic_file, timing::snap};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub time: f64,
    pub color: String,
}

pub struct ProjectConfig {
    target: PathBuf,
    original: Vec<u8>,
    value: Value,
}

fn valid_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}

impl ProjectConfig {
    pub fn video_bitrate(&self, scene: &str) -> Result<Option<u32>> {
        self.bitrate(scene, "video")
    }

    pub fn set_export_audio_bitrate(&mut self, scene: &str, bitrate: u32) -> Result<()> {
        if scene.is_empty() || bitrate == 0 {
            return Err("Export audio bitrate requires a scene id and a positive bitrate".into());
        }
        let mut audio = &mut self.value;
        for key in ["diffusion", "export", scene, "audio"] {
            if !audio.is_object() {
                *audio = json!({});
            }
            audio = audio
                .as_object_mut()
                .unwrap()
                .entry(key)
                .or_insert_with(|| json!({}));
        }
        if !audio.is_object() {
            *audio = json!({});
        }
        audio
            .as_object_mut()
            .unwrap()
            .insert("bitrate".into(), json!(bitrate));
        Ok(())
    }

    pub fn video_enabled(&self, scene: &str) -> Result<bool> {
        match self
            .value
            .pointer("/diffusion/export")
            .and_then(|scenes| scenes.get(scene))
            .and_then(|settings| settings.get("video"))
            .and_then(|video| video.get("enabled"))
        {
            None => Ok(true),
            Some(value) => value
                .as_bool()
                .ok_or_else(|| "Video enabled must be a boolean".into()),
        }
    }

    pub fn audio_enabled(&self, scene: &str) -> Result<bool> {
        let value = self
            .value
            .pointer("/diffusion/export")
            .and_then(|scenes| scenes.get(scene))
            .and_then(|settings| settings.get("audio"))
            .and_then(|audio| audio.get("enabled"));
        value.map_or(Ok(true), |value| {
            value
                .as_bool()
                .ok_or_else(|| "Audio enabled must be a boolean".into())
        })
    }

    pub fn export_frame_rate(&self, scene: &str) -> Result<f64> {
        let value = self
            .value
            .pointer("/diffusion/export")
            .and_then(|scenes| scenes.get(scene))
            .and_then(|settings| settings.get("video"))
            .and_then(|video| video.get("fps"));
        let Some(value) = value else {
            return Ok(30.0);
        };
        let fps = value.as_f64().ok_or("Export FPS must be a number")?;
        if !fps.is_finite() || fps <= 0.0 || fps > 240.0 {
            return Err("Export FPS must be in (0, 240]".into());
        }
        Ok(fps)
    }

    pub fn audio_bitrate(&self, scene: &str) -> Result<Option<u32>> {
        self.bitrate(scene, "audio")
    }

    pub fn audio_encoding_bitrate(&self, scene: &str, extension: &str) -> Result<Option<u32>> {
        if self.audio_codec(scene, extension)? == "flac" {
            Ok(None)
        } else {
            self.audio_bitrate(scene)
        }
    }

    pub fn audio_codec(&self, scene: &str, extension: &str) -> Result<String> {
        let value = self
            .value
            .pointer("/diffusion/export")
            .and_then(|scenes| scenes.get(scene))
            .and_then(|settings| settings.get("audio"))
            .and_then(|audio| audio.get("codec"));
        let codec = match value {
            None => {
                if matches!(extension, "webm" | "ogg") {
                    "opus"
                } else {
                    "aac"
                }
            }
            Some(value) => value.as_str().ok_or("Audio codec must be a string")?,
        };
        Self::validate_audio_codec(codec, extension)?;
        Ok(codec.into())
    }

    pub fn validate_audio_codec(codec: &str, extension: &str) -> Result<()> {
        if !(matches!(extension, "mp4" | "mov") && matches!(codec, "aac" | "mp3")
            || matches!(extension, "mp4" | "webm" | "ogg") && codec == "opus"
            || matches!(extension, "mp4" | "ogg") && codec == "flac"
            || matches!(extension, "webm" | "ogg") && codec == "vorbis")
        {
            return Err(format!(
                "Audio codec {codec:?} is unsupported for {extension}"
            ));
        }
        Ok(())
    }

    pub fn set_export_audio_codec(
        &mut self,
        scene: &str,
        extension: &str,
        codec: Option<&str>,
    ) -> Result<()> {
        if scene.is_empty() {
            return Err("Export requires a durable scene id".into());
        }
        if let Some(codec) = codec {
            Self::validate_audio_codec(codec, extension)?;
        }
        let mut audio = &mut self.value;
        for key in ["diffusion", "export", scene, "audio"] {
            if !audio.is_object() {
                *audio = json!({});
            }
            audio = audio
                .as_object_mut()
                .unwrap()
                .entry(key)
                .or_insert_with(|| json!({}));
        }
        if !audio.is_object() {
            *audio = json!({});
        }
        let audio = audio.as_object_mut().unwrap();
        if let Some(codec) = codec {
            audio.insert("codec".into(), json!(codec));
        } else {
            audio.remove("codec");
        }
        Ok(())
    }

    pub fn set_export_audio_channels(&mut self, scene: &str, channels: u32) -> Result<()> {
        if scene.is_empty() || !matches!(channels, 1 | 2) {
            return Err(
                "Export audio requires a durable scene id and mono or stereo channels".into(),
            );
        }
        let mut audio = &mut self.value;
        for key in ["diffusion", "export", scene, "audio"] {
            if !audio.is_object() {
                *audio = json!({});
            }
            audio = audio
                .as_object_mut()
                .unwrap()
                .entry(key)
                .or_insert_with(|| json!({}));
        }
        if !audio.is_object() {
            *audio = json!({});
        }
        audio
            .as_object_mut()
            .unwrap()
            .insert("numberOfChannels".into(), json!(channels));
        Ok(())
    }

    pub fn audio_channels(&self, scene: &str) -> Result<u32> {
        let value = self
            .value
            .pointer("/diffusion/export")
            .and_then(|scenes| scenes.get(scene))
            .and_then(|settings| settings.get("audio"))
            .and_then(|audio| audio.get("numberOfChannels"));
        match value {
            None => Ok(2),
            Some(value) if value.as_f64() == Some(1.0) => Ok(1),
            Some(value) if value.as_f64() == Some(2.0) => Ok(2),
            _ => Err("Audio numberOfChannels currently supports mono (1) and stereo (2); surround mixing is pending".into()),
        }
    }

    pub fn video_codec(&self, scene: &str, extension: &str) -> Result<String> {
        let value = self
            .value
            .pointer("/diffusion/export")
            .and_then(|scenes| scenes.get(scene))
            .and_then(|settings| settings.get("video"))
            .and_then(|video| video.get("codec"));
        let codec = match value {
            None => {
                if extension == "webm" {
                    "vp9"
                } else {
                    "avc"
                }
            }
            Some(value) => value.as_str().ok_or("Video codec must be a string")?,
        };
        Self::validate_video_codec(codec, extension)?;
        Ok(codec.into())
    }

    pub fn validate_video_codec(codec: &str, extension: &str) -> Result<()> {
        if !(extension == "webm" && matches!(codec, "vp8" | "vp9" | "av1")
            || matches!(extension, "mp4" | "mov") && matches!(codec, "avc" | "hevc")
            || extension == "mp4" && codec == "av1")
        {
            return Err(format!(
                "Video codec {codec:?} is unsupported for {extension}; additional codecs are pending"
            ));
        }
        Ok(())
    }

    pub fn set_export_video_codec(
        &mut self,
        scene: &str,
        extension: &str,
        codec: Option<&str>,
    ) -> Result<()> {
        if scene.is_empty() {
            return Err("Export requires a durable scene id".into());
        }
        if let Some(codec) = codec {
            Self::validate_video_codec(codec, extension)?;
        }
        let mut video = &mut self.value;
        for key in ["diffusion", "export", scene, "video"] {
            if !video.is_object() {
                *video = json!({});
            }
            video = video
                .as_object_mut()
                .unwrap()
                .entry(key)
                .or_insert_with(|| json!({}));
        }
        if !video.is_object() {
            *video = json!({});
        }
        let video = video.as_object_mut().unwrap();
        if let Some(codec) = codec {
            video.insert("codec".into(), json!(codec));
        } else {
            video.remove("codec");
        }
        Ok(())
    }

    pub fn video_resolution(&self, scene: &str) -> Result<Option<u32>> {
        let value = self
            .value
            .pointer("/diffusion/export")
            .and_then(|scenes| scenes.get(scene))
            .and_then(|settings| settings.get("video"))
            .and_then(|video| video.get("resolution"));
        let Some(value) = value else {
            return Ok(None);
        };
        let height = value
            .as_f64()
            .ok_or("Video resolution must be a height in pixels")?;
        if !height.is_finite() || !(1.0..=8192.0).contains(&height) || height.fract() != 0.0 {
            return Err(
                "Video resolution must be an integer height between 1 and 8192 pixels".into(),
            );
        }
        Ok(Some(height as u32))
    }

    pub fn audio_sample_rate(&self, scene: &str, extension: &str) -> Result<u32> {
        let value = self
            .value
            .pointer("/diffusion/export")
            .and_then(|scenes| scenes.get(scene))
            .and_then(|settings| settings.get("audio"))
            .and_then(|audio| audio.get("sampleRate"));
        let Some(value) = value else {
            return Ok(48_000);
        };
        let rate = value.as_f64().ok_or("Audio sampleRate must be a number")?;
        let codec = self.audio_codec(scene, extension)?;
        Self::validate_audio_sample_rate(rate, &codec)
    }

    pub fn validate_audio_sample_rate(rate: f64, codec: &str) -> Result<u32> {
        if codec == "flac" {
            if !rate.is_finite() || rate.fract() != 0.0 || !(8000.0..=192000.0).contains(&rate) {
                return Err("FLAC sampleRate must be an integer between 8000 and 192000 Hz".into());
            }
            return Ok(rate as u32);
        }
        let supported: &[u32] = match codec {
            "opus" => &[8000, 12000, 16000, 24000, 48000],
            "mp3" => &[8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000],
            _ => &[
                7350, 8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200,
                96000,
            ],
        };
        if !supported.iter().any(|candidate| *candidate as f64 == rate) {
            return Err(format!(
                "Audio sampleRate {rate} is unsupported for {codec}"
            ));
        }
        Ok(rate as u32)
    }

    pub fn set_export_audio_sample_rate(
        &mut self,
        scene: &str,
        extension: &str,
        rate: u32,
    ) -> Result<()> {
        if scene.is_empty() {
            return Err("Export requires a durable scene id".into());
        }
        Self::validate_audio_sample_rate(rate as f64, &self.audio_codec(scene, extension)?)?;
        let mut audio = &mut self.value;
        for key in ["diffusion", "export", scene, "audio"] {
            if !audio.is_object() {
                *audio = json!({});
            }
            audio = audio
                .as_object_mut()
                .unwrap()
                .entry(key)
                .or_insert_with(|| json!({}));
        }
        if !audio.is_object() {
            *audio = json!({});
        }
        audio
            .as_object_mut()
            .unwrap()
            .insert("sampleRate".into(), json!(rate));
        Ok(())
    }

    fn bitrate(&self, scene: &str, section: &str) -> Result<Option<u32>> {
        let Some(value) = self
            .value
            .pointer("/diffusion/export")
            .and_then(|scenes| scenes.get(scene))
            .and_then(|settings| settings.get(section))
            .and_then(|settings| settings.get("bitrate"))
        else {
            return Ok(None);
        };
        let number = value
            .as_f64()
            .ok_or_else(|| format!("{section} bitrate must be a number in bits per second"))?;
        if !number.is_finite() || number < 1.0 || number > u32::MAX as f64 {
            return Err(format!(
                "{section} bitrate must be between 1 and 4294967295 bits per second"
            ));
        }
        Ok(Some(number.round() as u32))
    }

    pub fn clip_color(&self, source_key: &str) -> Option<&str> {
        self.value
            .get("diffusion")?
            .get("clipColors")?
            .get(source_key)?
            .as_str()
            .filter(|color| valid_color(color))
    }

    pub fn set_clip_colors(&mut self, source_keys: &[String], color: Option<&str>) -> Result<()> {
        if color.is_some_and(|color| !valid_color(color))
            || source_keys.iter().any(|key| key.is_empty())
        {
            return Err("Clip colors require source keys and a six-digit hex color".into());
        }
        if source_keys.is_empty() {
            return Ok(());
        }
        let config = self
            .value
            .as_object_mut()
            .unwrap()
            .entry("diffusion")
            .or_insert_with(|| json!({}));
        if !config.is_object() {
            *config = json!({});
        }
        let config = config.as_object_mut().unwrap();
        let colors = config.entry("clipColors").or_insert_with(|| json!({}));
        if !colors.is_object() {
            *colors = json!({});
        }
        let colors = colors.as_object_mut().unwrap();
        for key in source_keys {
            if let Some(color) = color {
                colors.insert(key.clone(), json!(color));
            } else {
                colors.remove(key);
            }
        }
        if colors.is_empty() {
            config.remove("clipColors");
        }
        Ok(())
    }

    pub fn set_export_audio(&mut self, scene: &str, enabled: bool) -> Result<()> {
        if scene.is_empty() {
            return Err("Export requires a durable scene id".into());
        }
        fn object(value: &mut Value) -> &mut serde_json::Map<String, Value> {
            if !value.is_object() {
                *value = json!({});
            }
            value.as_object_mut().unwrap()
        }
        let config = object(
            self.value
                .as_object_mut()
                .unwrap()
                .entry("diffusion")
                .or_insert_with(|| json!({})),
        );
        let exports = object(config.entry("export").or_insert_with(|| json!({})));
        let settings = object(exports.entry(scene).or_insert_with(|| json!({})));
        object(settings.entry("audio").or_insert_with(|| json!({})))
            .insert("enabled".into(), json!(enabled));
        Ok(())
    }

    pub fn set_export_video_bitrate(&mut self, scene: &str, bitrate: u32) -> Result<()> {
        if scene.is_empty() || bitrate == 0 {
            return Err("Export video bitrate requires a scene id and a positive bitrate".into());
        }
        let mut video = &mut self.value;
        for key in ["diffusion", "export", scene, "video"] {
            if !video.is_object() {
                *video = json!({});
            }
            video = video
                .as_object_mut()
                .unwrap()
                .entry(key)
                .or_insert_with(|| json!({}));
        }
        if !video.is_object() {
            *video = json!({});
        }
        video
            .as_object_mut()
            .unwrap()
            .insert("bitrate".into(), json!(bitrate));
        Ok(())
    }

    pub fn set_export_resolution(&mut self, scene: &str, height: u32) -> Result<()> {
        if scene.is_empty() || !(1..=8192).contains(&height) {
            return Err(
                "Export resolution requires a scene id and height between 1 and 8192".into(),
            );
        }
        let mut settings = &mut self.value;
        for key in ["diffusion", "export", scene, "video"] {
            if !settings.is_object() {
                *settings = json!({});
            }
            settings = settings
                .as_object_mut()
                .unwrap()
                .entry(key)
                .or_insert_with(|| json!({}));
        }
        if !settings.is_object() {
            *settings = json!({});
        }
        settings
            .as_object_mut()
            .unwrap()
            .insert("resolution".into(), json!(height));
        Ok(())
    }

    pub fn set_export_video(&mut self, scene: &str, enabled: bool) -> Result<()> {
        if scene.is_empty() {
            return Err("Export requires a durable scene id".into());
        }
        let mut settings = &mut self.value;
        for key in ["diffusion", "export", scene, "video"] {
            if !settings.is_object() {
                *settings = json!({});
            }
            settings = settings
                .as_object_mut()
                .unwrap()
                .entry(key)
                .or_insert_with(|| json!({}));
        }
        if !settings.is_object() {
            *settings = json!({});
        }
        settings
            .as_object_mut()
            .unwrap()
            .insert("enabled".into(), json!(enabled));
        Ok(())
    }

    pub fn set_export_timing(&mut self, scene: &str, format: &str, fps: f64) -> Result<()> {
        if scene.is_empty()
            || !matches!(format, "mp4" | "mov" | "webm" | "ogg")
            || !fps.is_finite()
            || fps <= 0.0
            || fps > 240.0
        {
            return Err("Invalid scene export format or frame rate".into());
        }
        fn object(value: &mut Value) -> &mut serde_json::Map<String, Value> {
            if !value.is_object() {
                *value = json!({});
            }
            value.as_object_mut().unwrap()
        }
        let config = object(
            self.value
                .as_object_mut()
                .unwrap()
                .entry("diffusion")
                .or_insert_with(|| json!({})),
        );
        let exports = object(config.entry("export").or_insert_with(|| json!({})));
        let settings = object(exports.entry(scene).or_insert_with(|| json!({})));
        settings.insert("format".into(), json!(format));
        let video = object(settings.entry("video").or_insert_with(|| json!({})));
        video.insert("fps".into(), json!(fps));
        Ok(())
    }

    /// Sanitized authored export settings; unsupported native encoder options
    /// remain visible to callers instead of being silently substituted.
    pub fn export_settings(&self, scene: &str) -> Value {
        let Some(raw) = self
            .value
            .get("diffusion")
            .and_then(|v| v.get("export"))
            .and_then(|v| v.get(scene))
            .and_then(Value::as_object)
        else {
            return json!({});
        };
        let mut result = serde_json::Map::new();
        if let Some(format) = raw
            .get("format")
            .and_then(Value::as_str)
            .filter(|format| !format.is_empty())
        {
            result.insert("format".into(), json!(format));
        }
        if let Some(template) = raw
            .get("template")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            result.insert("template".into(), json!(template));
        }
        for (section, numbers) in [
            ("video", &["fps", "resolution", "bitrate"][..]),
            ("audio", &["sampleRate", "bitrate", "numberOfChannels"][..]),
        ] {
            if let Some(settings) = raw.get(section).and_then(Value::as_object) {
                let mut parsed = serde_json::Map::new();
                if let Some(enabled) = settings.get("enabled").and_then(Value::as_bool) {
                    parsed.insert("enabled".into(), json!(enabled));
                }
                if let Some(codec) = settings
                    .get("codec")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                {
                    parsed.insert("codec".into(), json!(codec));
                }
                for name in numbers {
                    if let Some(number) = settings
                        .get(*name)
                        .and_then(Value::as_f64)
                        .filter(|v| v.is_finite())
                    {
                        parsed.insert((*name).into(), json!(number));
                    }
                }
                result.insert(section.into(), Value::Object(parsed));
            }
        }
        Value::Object(result)
    }

    pub fn load(project: &Path) -> Result<Self> {
        let target = atomic_file::source_target(project, &project.join("package.json"))?;
        let original = fs::read(&target).map_err(|e| e.to_string())?;
        let value: Value = serde_json::from_slice(&original).map_err(|e| e.to_string())?;
        if !value.is_object() {
            return Err("Project package.json must be an object".into());
        }
        Ok(Self {
            target,
            original,
            value,
        })
    }

    pub fn markers(&self, scene: &str) -> Vec<Marker> {
        self.value
            .get("diffusion")
            .and_then(|v| v.get("markers"))
            .and_then(|v| v.get(scene))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| serde_json::from_value::<Marker>(value.clone()).ok())
            .filter(|marker| {
                marker.time.is_finite() && marker.time >= 0.0 && valid_color(&marker.color)
            })
            .collect()
    }

    pub fn set_markers(&mut self, scene: &str, markers: &[Marker]) -> Result<()> {
        if scene.is_empty() {
            return Err("Markers require a durable scene id".into());
        }
        if markers
            .iter()
            .any(|m| !m.time.is_finite() || m.time < 0.0 || !valid_color(&m.color))
        {
            return Err("Invalid timeline marker".into());
        }
        let root = self.value.as_object_mut().unwrap();
        let config = root.entry("diffusion").or_insert_with(|| json!({}));
        if !config.is_object() {
            *config = json!({});
        }
        let config = config.as_object_mut().unwrap();
        let all = config.entry("markers").or_insert_with(|| json!({}));
        if !all.is_object() {
            *all = json!({});
        }
        let all = all.as_object_mut().unwrap();
        if markers.is_empty() {
            all.remove(scene);
        } else {
            all.insert(
                scene.into(),
                serde_json::to_value(markers).map_err(|e| e.to_string())?,
            );
        }
        if all.is_empty() {
            config.remove("markers");
        }
        Ok(())
    }

    pub fn toggle_marker(&mut self, scene: &str, time: f64, color: &str) -> Result<()> {
        if !time.is_finite() || !valid_color(color) {
            return Err("Invalid timeline marker".into());
        }
        let time = snap(time.max(0.0));
        let mut markers = self.markers(scene);
        if markers.iter().any(|m| snap(m.time) == time) {
            markers.retain(|m| snap(m.time) != time);
        } else {
            markers.push(Marker {
                time,
                color: color.into(),
            });
        }
        self.set_markers(scene, &markers)
    }

    pub fn save(&mut self) -> Result<()> {
        let directory = self.target.parent().ok_or("Invalid configuration path")?;
        let state = directory.join(".diffusion");
        fs::create_dir_all(&state).map_err(|e| e.to_string())?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(state.join("native-config.lock"))
            .map_err(|e| e.to_string())?;
        lock.lock().map_err(|e| e.to_string())?;
        if fs::read(&self.target).map_err(|e| e.to_string())? != self.original {
            return Err("Project configuration changed externally; reload before saving".into());
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let source = format!(
            "{}\n",
            serde_json::to_string_pretty(&self.value).map_err(|e| e.to_string())?
        );
        let temp = self.target.with_file_name(format!(
            ".ad3-config-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        atomic_file::write_candidate(&temp, &self.target, &self.original, &source)?;
        self.original = source.into_bytes();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_fps_and_audio_preferences_validate_saved_values() {
        let mut config = ProjectConfig {
            target: PathBuf::new(),
            original: Vec::new(),
            value: json!({"diffusion":{"export":{"s":{"video":{},"audio":{}}}}}),
        };
        assert_eq!(config.export_frame_rate("s").unwrap(), 30.0);
        assert!(config.audio_enabled("s").unwrap());
        config.value["diffusion"]["export"]["s"]["audio"]["enabled"] = json!(false);
        assert!(!config.audio_enabled("s").unwrap());
        config.value["diffusion"]["export"]["s"]["audio"]["enabled"] = json!("false");
        assert!(config.audio_enabled("s").is_err());
        for (value, expected) in [
            (json!(23.976), Some(23.976)),
            (json!(240), Some(240.0)),
            (json!(0), None),
            (json!(241), None),
            (json!("60"), None),
        ] {
            config.value["diffusion"]["export"]["s"]["video"]["fps"] = value;
            assert_eq!(config.export_frame_rate("s").ok(), expected);
        }
    }
    #[test]
    fn video_enabled_defaults_to_true_and_requires_boolean() {
        let mut config = ProjectConfig {
            target: PathBuf::new(),
            original: Vec::new(),
            value: json!({"diffusion":{"export":{"s":{"video":{}}}}}),
        };
        assert!(config.video_enabled("s").unwrap());
        config.value["diffusion"]["export"]["s"]["video"]["enabled"] = json!(false);
        assert!(!config.video_enabled("s").unwrap());
        config.value["diffusion"]["export"]["s"]["video"]["enabled"] = json!("false");
        assert!(config.video_enabled("s").is_err());
    }
    #[test]
    fn audio_codecs_validate_container_and_sample_rate() {
        let mut config = ProjectConfig {
            target: PathBuf::new(),
            original: Vec::new(),
            value: json!({"diffusion":{"export":{"s":{"audio":{}}}}}),
        };
        assert_eq!(config.audio_codec("s", "mp4").unwrap(), "aac");
        assert_eq!(config.audio_codec("s", "webm").unwrap(), "opus");
        config.value["diffusion"]["export"]["s"]["audio"] =
            json!({"codec":"opus","sampleRate":44100});
        assert!(config.audio_codec("s", "mp4").is_ok());
        assert!(config.audio_codec("s", "mov").is_err());
        assert!(config.audio_sample_rate("s", "mp4").is_err());
        config.value["diffusion"]["export"]["s"]["audio"]["sampleRate"] = json!(24000);
        assert_eq!(config.audio_sample_rate("s", "mp4").unwrap(), 24000);
        config.value["diffusion"]["export"]["s"]["audio"]["codec"] = json!("aac");
        assert!(config.audio_codec("s", "webm").is_err());
        config.value["diffusion"]["export"]["s"]["audio"] =
            json!({"codec":"mp3","sampleRate":44100});
        assert!(config.audio_codec("s", "mp4").is_ok());
        assert!(config.audio_codec("s", "mov").is_ok());
        assert!(config.audio_codec("s", "webm").is_err());
        assert_eq!(config.audio_sample_rate("s", "mp4").unwrap(), 44100);
        config.value["diffusion"]["export"]["s"]["audio"]["sampleRate"] = json!(96000);
        assert!(config.audio_sample_rate("s", "mp4").is_err());
        config.value["diffusion"]["export"]["s"]["audio"]["codec"] = json!("flac");
        assert_eq!(config.audio_sample_rate("s", "mp4").unwrap(), 96000);
        assert!(config.audio_codec("s", "ogg").is_ok());
        config.value["diffusion"]["export"]["s"]["audio"]["bitrate"] = json!("unused");
        assert!(config.audio_bitrate("s").is_err());
        assert_eq!(config.audio_encoding_bitrate("s", "mp4").unwrap(), None);
        assert_eq!(config.audio_encoding_bitrate("s", "ogg").unwrap(), None);
        assert!(config.audio_codec("s", "mov").is_err());
        assert!(config.audio_codec("s", "webm").is_err());
        for rate in [json!(7999), json!(192001), json!(96000.5)] {
            config.value["diffusion"]["export"]["s"]["audio"]["sampleRate"] = rate;
            assert!(config.audio_sample_rate("s", "mp4").is_err());
        }
        config.value["diffusion"]["export"]["s"]["audio"] =
            json!({"codec":"vorbis","sampleRate":44100});
        assert!(config.audio_codec("s", "ogg").is_ok());
        assert!(config.audio_codec("s", "webm").is_ok());
        assert!(config.audio_codec("s", "mp4").is_err());
        assert_eq!(config.audio_sample_rate("s", "ogg").unwrap(), 44100);
    }
    #[test]
    fn video_codecs_validate_saved_container_and_defaults() {
        let mut config = ProjectConfig {
            target: PathBuf::new(),
            original: Vec::new(),
            value: json!({"diffusion":{"export":{"s":{"video":{}}}}}),
        };
        assert_eq!(config.video_codec("s", "mp4").unwrap(), "avc");
        assert_eq!(config.video_codec("s", "webm").unwrap(), "vp9");
        for codec in ["vp8", "vp9"] {
            config.value["diffusion"]["export"]["s"]["video"]["codec"] = json!(codec);
            assert_eq!(config.video_codec("s", "webm").unwrap(), codec);
            assert!(config.video_codec("s", "mov").is_err());
        }
        config.value["diffusion"]["export"]["s"]["video"]["codec"] = json!("avc");
        assert!(config.video_codec("s", "mov").is_ok());
        assert!(config.video_codec("s", "webm").is_err());
        config.value["diffusion"]["export"]["s"]["video"]["codec"] = json!("hevc");
        assert!(config.video_codec("s", "mp4").is_ok());
        assert!(config.video_codec("s", "mov").is_ok());
        assert!(config.video_codec("s", "webm").is_err());
        config.value["diffusion"]["export"]["s"]["video"]["codec"] = json!("av1");
        for extension in ["mp4", "webm"] {
            assert_eq!(config.video_codec("s", extension).unwrap(), "av1");
        }
        assert!(config.video_codec("s", "mov").is_err());
        for codec in [json!("unknown"), json!(1), Value::Null] {
            config.value["diffusion"]["export"]["s"]["video"]["codec"] = codec;
            assert!(config.video_codec("s", "mp4").is_err());
        }
    }
    #[test]
    fn audio_channel_count_defaults_to_stereo_and_validates_mono() {
        let mut config = ProjectConfig {
            target: PathBuf::new(),
            original: Vec::new(),
            value: json!({"diffusion":{"export":{"s":{"audio":{}}}}}),
        };
        assert_eq!(config.audio_channels("s").unwrap(), 2);
        for (value, expected) in [
            (json!(1), Some(1)),
            (json!(2.0), Some(2)),
            (json!(0), None),
            (json!(6), None),
            (json!(1.5), None),
            (json!("1"), None),
            (Value::Null, None),
        ] {
            config.value["diffusion"]["export"]["s"]["audio"]["numberOfChannels"] = value;
            assert_eq!(config.audio_channels("s").ok(), expected);
        }
    }
    #[test]
    fn video_resolution_is_height_and_invalid_values_are_rejected() {
        let mut config = ProjectConfig {
            target: PathBuf::new(),
            original: Vec::new(),
            value: json!({"diffusion":{"export":{"s":{"video":{"resolution":720}}}}}),
        };
        assert_eq!(config.video_resolution("s").unwrap(), Some(720));
        assert_eq!(config.video_resolution("other").unwrap(), None);
        config.value["diffusion"]["export"]["s"]["video"]["codec"] = json!("avc");
        config.set_export_resolution("s", 540).unwrap();
        assert_eq!(config.video_resolution("s").unwrap(), Some(540));
        assert_eq!(
            config.value["diffusion"]["export"]["s"]["video"]["codec"],
            "avc"
        );
        let before = config.value.clone();
        assert!(config.set_export_resolution("s", 0).is_err());
        assert!(config.set_export_resolution("", 1080).is_err());
        assert_eq!(config.value, before);
        for value in [
            json!(0),
            json!(-1),
            json!(8193),
            json!(0.5),
            json!("720"),
            Value::Null,
        ] {
            config.value["diffusion"]["export"]["s"]["video"]["resolution"] = value;
            assert!(config.video_resolution("s").is_err());
        }
    }
    #[test]
    fn audio_sample_rates_validate_codec_and_keep_default() {
        let mut config = ProjectConfig {
            target: PathBuf::new(),
            original: Vec::new(),
            value: json!({"diffusion":{"export":{"s":{"audio":{}}}}}),
        };
        assert_eq!(config.audio_sample_rate("s", "mp4").unwrap(), 48000);
        assert_eq!(config.audio_sample_rate("other", "webm").unwrap(), 48000);
        for (value, aac, opus) in [
            (json!(44100), true, false),
            (json!(24000), true, true),
            (json!(96000), true, false),
            (json!(44100.5), false, false),
            (json!(0), false, false),
            (json!("48000"), false, false),
            (Value::Null, false, false),
        ] {
            config.value["diffusion"]["export"]["s"]["audio"]["sampleRate"] = value;
            assert_eq!(config.audio_sample_rate("s", "mov").is_ok(), aac);
            assert_eq!(config.audio_sample_rate("s", "webm").is_ok(), opus);
        }
    }
    #[test]
    fn video_bitrate_reads_scene_and_rejects_invalid_saved_values() {
        let mut config = ProjectConfig {
            target: PathBuf::new(),
            original: Vec::new(),
            value: json!({"diffusion":{"export":{"s":{"video":{"bitrate":500000.4}}}}}),
        };
        assert_eq!(config.video_bitrate("s").unwrap(), Some(500000));
        assert_eq!(config.video_bitrate("other").unwrap(), None);
        assert_eq!(config.audio_bitrate("s").unwrap(), None);
        config.value["diffusion"]["export"]["s"]["audio"] = json!({"bitrate":64000});
        assert_eq!(config.audio_bitrate("s").unwrap(), Some(64000));
        for value in [
            json!(0),
            json!(-1),
            json!(4294967296u64),
            json!("bad"),
            Value::Null,
        ] {
            config.value["diffusion"]["export"]["s"]["video"]["bitrate"] = value;
            assert!(config.video_bitrate("s").is_err());
            config.value["diffusion"]["export"]["s"]["audio"]["bitrate"] =
                config.value["diffusion"]["export"]["s"]["video"]["bitrate"].clone();
            assert!(config.audio_bitrate("s").is_err());
        }
    }
    #[test]
    fn clip_colors_use_full_source_keys_and_preserve_other_metadata() {
        let mut config = ProjectConfig {
            target: PathBuf::new(),
            original: Vec::new(),
            value: json!({"diffusion":{"clipColors":{"/index.tsx:a":"#3B82F6","/other.tsx:a":"#10B981","invalid":"blue"},"export":{"s":{"format":"mov"}}}}),
        };
        assert_eq!(config.clip_color("/index.tsx:a"), Some("#3B82F6"));
        assert_eq!(config.clip_color("invalid"), None);
        config
            .set_clip_colors(
                &["/index.tsx:a".into(), "/index.tsx:b".into()],
                Some("#EC4899"),
            )
            .unwrap();
        assert_eq!(config.clip_color("/index.tsx:b"), Some("#EC4899"));
        config
            .set_clip_colors(&["/index.tsx:a".into()], None)
            .unwrap();
        assert_eq!(config.clip_color("/index.tsx:a"), None);
        assert_eq!(config.clip_color("/other.tsx:a"), Some("#10B981"));
        assert_eq!(config.export_settings("s")["format"], "mov");
        let before = config.value.clone();
        assert!(
            config
                .set_clip_colors(&["/index.tsx:b".into()], Some("pink"))
                .is_err()
        );
        assert_eq!(config.value, before);
    }

    #[test]
    fn export_timing_writes_preserve_other_encoder_settings_and_scenes() {
        let mut config = ProjectConfig {
            target: PathBuf::new(),
            original: Vec::new(),
            value: json!({"name":"project", "diffusion":{"export":{"s":{"template":"custom","video":{"codec":"avc","bitrate":5000000,"resolution":0.5},"audio":{"codec":"aac","enabled":false}},"other":{"format":"webm"}},"markers":{"s":[{"time":1,"color":"#ffffff"}]}}}),
        };
        config.set_export_timing("s", "mov", 23.976).unwrap();
        assert_eq!(config.export_settings("s")["video"]["codec"], "avc");
        assert_eq!(config.export_settings("s")["video"]["resolution"], 0.5);
        assert_eq!(config.export_settings("s")["audio"]["enabled"], false);
        assert_eq!(config.export_settings("other")["format"], "webm");
        assert_eq!(config.markers("s").len(), 1);
        let before = config.value.clone();
        assert!(config.set_export_timing("s", "mov", f64::NAN).is_err());
        assert_eq!(config.value, before);
    }

    #[test]
    fn concurrent_marker_writers_commit_once_and_keep_the_winner() {
        let base = std::env::temp_dir();
        let folder = base.join(format!(
            "ad3-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("package.json"), r#"{"name":"concurrent"}"#).unwrap();
        let mut first = ProjectConfig::load(&folder).unwrap();
        let mut second = ProjectConfig::load(&folder).unwrap();
        first.toggle_marker("s", 1.0, "#ffffff").unwrap();
        second.toggle_marker("s", 2.0, "#ffffff").unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let b = barrier.clone();
        let a = std::thread::spawn(move || {
            b.wait();
            first.save()
        });
        let b = std::thread::spawn(move || {
            barrier.wait();
            second.save()
        });
        let first_result = a.join().unwrap();
        let second_result = b.join().unwrap();
        assert_ne!(first_result.is_ok(), second_result.is_ok());
        let markers = ProjectConfig::load(&folder).unwrap().markers("s");
        assert_eq!(markers.len(), 1);
        assert_eq!(
            markers[0].time,
            if first_result.is_ok() { 1.0 } else { 2.0 }
        );
        let resolved = fs::canonicalize(&folder).unwrap();
        assert!(resolved.starts_with(fs::canonicalize(&base).unwrap()));
        assert!(
            resolved
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("ad3-config-test-")
        );
        fs::remove_dir_all(resolved).unwrap();
    }

    #[test]
    fn marker_save_reopens_and_external_changes_are_preserved() {
        let base = std::env::temp_dir();
        let folder = base.join(format!(
            "ad3-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&folder).unwrap();
        let target = folder.join("package.json");
        fs::write(
            &target,
            r#"{"name":"test","diffusion":{"export":{"s":{"format":"mov"}}}}"#,
        )
        .unwrap();
        let mut config = ProjectConfig::load(&folder).unwrap();
        config.toggle_marker("s", 1.0, "#EC4899").unwrap();
        config.save().unwrap();
        assert_eq!(ProjectConfig::load(&folder).unwrap().markers("s").len(), 1);
        let external = r#"{"name":"external"}"#;
        fs::write(&target, external).unwrap();
        config.toggle_marker("s", 2.0, "#ffffff").unwrap();
        assert!(config.save().is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), external);
        let resolved = fs::canonicalize(&folder).unwrap();
        assert!(resolved.starts_with(fs::canonicalize(&base).unwrap()));
        assert!(
            resolved
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("ad3-config-test-")
        );
        fs::remove_dir_all(resolved).unwrap();
    }

    #[test]
    fn markers_filter_toggle_and_preserve_unrelated_metadata() {
        let mut config = ProjectConfig {
            target: PathBuf::new(),
            original: Vec::new(),
            value: json!({"name":"project", "scripts":{"build":"existing"}, "diffusion":{"export":{"s":{"format":"mov"}}, "markers":{"other":[{"time":3,"color":"#123456"}], "s":[{"time":1,"color":"#EC4899"},{"time":-1,"color":"#ffffff"},{"time":2,"color":"red"}]}}}),
        };
        assert_eq!(config.markers("s").len(), 1);
        config.toggle_marker("s", 1.001, "#EC4899").unwrap();
        assert!(config.markers("s").is_empty());
        config.toggle_marker("s", 2.01, "#123ABC").unwrap();
        assert_eq!(config.markers("s")[0].time, 2.0);
        config.set_markers("s", &[]).unwrap();
        assert_eq!(config.value["name"], "project");
        assert_eq!(config.value["scripts"]["build"], "existing");
        assert_eq!(config.value["diffusion"]["export"]["s"]["format"], "mov");
        assert_eq!(config.export_settings("s"), json!({"format":"mov"}));
        assert_eq!(config.export_settings("missing"), json!({}));
        assert_eq!(config.markers("other").len(), 1);
        assert!(config.toggle_marker("s", f64::NAN, "#ffffff").is_err());
        assert!(config.toggle_marker("s", 2.0, "pink").is_err());
    }

    #[test]
    fn export_settings_ignore_invalid_fields_without_losing_valid_siblings() {
        let config = ProjectConfig {
            target: PathBuf::new(),
            original: Vec::new(),
            value: json!({"diffusion":{"export":{"s":{"format":"webm","template":"custom","video":{"fps":60,"resolution":0.5,"bitrate":"bad","codec":"vp9","enabled":false},"audio":{"sampleRate":48000,"bitrate":128000,"enabled":"bad","codec":"opus"}},"invalid":{"format":"avi","video":[]}}}}),
        };
        assert_eq!(
            config.export_settings("s"),
            json!({"format":"webm","template":"custom","video":{"fps":60.0,"resolution":0.5,"codec":"vp9","enabled":false},"audio":{"sampleRate":48000.0,"bitrate":128000.0,"codec":"opus"}})
        );
        assert_eq!(config.export_settings("invalid"), json!({"format":"avi"}));
    }
}
