//! Voice tool — text-to-speech and speech-to-text.
//!
//! TTS providers (tried in order):
//!   1. edge   — Microsoft Edge TTS (free, cloud, no API key, high quality)
//!   2. openai — OpenAI TTS (needs OPENAI_API_KEY)
//!   3. espeak-ng / flite / say — local engines
//!
//! STT: Whisper local or OpenAI Whisper API.

use super::{Tool, ToolOutput};
use async_trait::async_trait;

pub struct VoiceTool;

#[async_trait]
impl Tool for VoiceTool {
    fn name(&self) -> &str {
        "text_to_speech"
    }

    fn description(&self) -> &str {
        "Convert text to speech (TTS). Generates an audio file (MP3) from text. \
         Providers: edge (free, best quality), openai (needs API key), local (espeak/flite/say). \
         Use output_path to control where the file is saved."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to convert to speech"
                },
                "output_path": {
                    "type": "string",
                    "description": "Path to save MP3/WAV file (default: ~/.arli/tts/tts_<timestamp>.mp3)"
                },
                "provider": {
                    "type": "string",
                    "enum": ["edge", "openai", "local", "auto"],
                    "description": "TTS provider: edge (free, best), openai (needs API key), local, auto (default: edge)"
                },
                "voice": {
                    "type": "string",
                    "description": "Voice name. Edge: en-US-JennyNeural, ru-RU-SvetlanaNeural, etc. OpenAI: alloy, echo, fable, onyx, nova, shimmer. Local: en, ru"
                },
                "speed": {
                    "type": "number",
                    "description": "Speech speed multiplier (0.5-2.0, default: 1.0)"
                }
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => return ToolOutput::error(&format!("Invalid JSON: {}", e)),
        };

        let text = match args["text"].as_str() {
            Some(t) => t,
            None => return ToolOutput::error("Missing 'text' parameter"),
        };

        let provider = args["provider"].as_str().unwrap_or("edge");
        let voice = args["voice"].as_str().unwrap_or("");
        let speed: f32 = args["speed"].as_f64().map(|v| v as f32).unwrap_or(1.0);

        let output_path = args["output_path"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| default_output_path());

        // Ensure output directory exists
        if let Some(parent) = std::path::Path::new(&output_path).parent() {
            std::fs::create_dir_all(parent).ok();
        }

        match provider {
            "edge" => tts_edge(text, &output_path, voice, speed).await,
            "openai" => tts_openai(text, &output_path, voice, speed).await,
            "local" => tts_local(text, &output_path, voice).await,
            "auto" | _ => {
                // Try Edge first, then OpenAI, then local
                let result = tts_edge(text, &output_path, voice, speed).await;
                if result.success {
                    return result;
                }
                let result = tts_openai(text, &output_path, voice, speed).await;
                if result.success {
                    return result;
                }
                tts_local(text, &output_path, voice).await
            }
        }
    }
}

fn default_output_path() -> String {
    let dir = std::env::var("ARLI_HOME")
        .map(|h| format!("{}/tts", h))
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| format!("{}/.arli/tts", h))
                .unwrap_or_else(|_| "/tmp".to_string())
        });
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    format!("{}/tts_{}.mp3", dir, ts)
}

// ── Edge TTS (Microsoft, free, cloud) ──

async fn tts_edge(text: &str, output_path: &str, voice: &str, speed: f32) -> ToolOutput {
    let voice_name = if voice.is_empty() {
        "en-US-JennyNeural"
    } else {
        voice
    };

    let rate = if (speed - 1.0).abs() > 0.01 {
        let pct = ((speed - 1.0) * 100.0) as i32;
        format!(" rate=\"{:+}%\"", pct)
    } else {
        String::new()
    };

    let ssml = format!(
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>\
         <voice name='{}'>\
         <prosody{} pitch='+0Hz'>\
         {}\
         </prosody>\
         </voice>\
         </speak>",
        voice_name,
        rate,
        escape_xml(text)
    );

    let client = reqwest::Client::new();
    let resp = match client
        .post("https://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1")
        .header("Content-Type", "application/ssml+xml")
        .header(
            "X-Microsoft-OutputFormat",
            "audio-16khz-128kbitrate-mono-mp3",
        )
        .header("User-Agent", "Mozilla/5.0")
        .body(ssml)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Edge TTS request failed: {}", e)),
            }
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return ToolOutput {
            success: false,
            content: String::new(),
            error: Some(format!("Edge TTS HTTP {}: {}", status, body)),
        };
    }

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("Edge TTS read response: {}", e)),
            }
        }
    };

    if bytes.len() < 100 {
        return ToolOutput {
            success: false,
            content: String::new(),
            error: Some("Edge TTS returned empty audio".into()),
        };
    }

    match std::fs::write(output_path, &bytes) {
        Ok(_) => ToolOutput {
            success: true,
            content: format!(
                "TTS generated (Edge: {})\nOutput: {}\nSize: {} bytes",
                voice_name,
                output_path,
                bytes.len()
            ),
            error: None,
        },
        Err(e) => ToolOutput {
            success: false,
            content: String::new(),
            error: Some(format!("Failed to write TTS file: {}", e)),
        },
    }
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ── OpenAI TTS ──

async fn tts_openai(text: &str, output_path: &str, voice: &str, speed: f32) -> ToolOutput {
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some("OPENAI_API_KEY not set".into()),
            }
        }
    };

    let voice_name = if voice.is_empty() { "alloy" } else { voice };

    let body = serde_json::json!({
        "model": "tts-1",
        "input": text,
        "voice": voice_name,
        "response_format": "mp3",
        "speed": speed
    });

    let client = reqwest::Client::new();
    let resp = match client
        .post("https://api.openai.com/v1/audio/speech")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("OpenAI TTS request failed: {}", e)),
            }
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let err_body = resp.text().await.unwrap_or_default();
        return ToolOutput {
            success: false,
            content: String::new(),
            error: Some(format!("OpenAI TTS HTTP {}: {}", status, err_body)),
        };
    }

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return ToolOutput {
                success: false,
                content: String::new(),
                error: Some(format!("OpenAI TTS read response: {}", e)),
            }
        }
    };

    match std::fs::write(output_path, &bytes) {
        Ok(_) => ToolOutput {
            success: true,
            content: format!(
                "TTS generated (OpenAI: {})\nOutput: {}\nSize: {} bytes",
                voice_name,
                output_path,
                bytes.len()
            ),
            error: None,
        },
        Err(e) => ToolOutput {
            success: false,
            content: String::new(),
            error: Some(format!("Failed to write TTS file: {}", e)),
        },
    }
}

// ── Local TTS (espeak-ng / flite / say) ──

async fn tts_local(text: &str, output_path: &str, voice: &str) -> ToolOutput {
    let voice = if voice.is_empty() { "en" } else { voice };
    let engine = detect_tts_engine();

    match engine {
        TtsEngine::EspeakNg => {
            let wav = if output_path.ends_with(".wav") {
                output_path.to_string()
            } else {
                format!("{}.wav", output_path.trim_end_matches(".mp3"))
            };
            let result = tokio::process::Command::new("espeak-ng")
                .args(["-v", voice, "-w", &wav, "--", text])
                .output()
                .await;
            match result {
                Ok(o) if o.status.success() => {
                    let size = std::fs::metadata(&wav).map(|m| m.len()).unwrap_or(0);
                    ToolOutput {
                        success: true,
                        content: format!(
                            "TTS (espeak-ng, {})\nOutput: {}\nSize: {} bytes",
                            voice, wav, size
                        ),
                        error: None,
                    }
                }
                Ok(o) => ToolOutput::error(&format!(
                    "espeak-ng failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                )),
                Err(e) => ToolOutput::error(&format!("espeak-ng error: {}", e)),
            }
        }
        TtsEngine::Flite => {
            let wav = format!("{}.wav", output_path.trim_end_matches(".mp3"));
            let result = tokio::process::Command::new("flite")
                .args(["-t", text, "-o", &wav])
                .output()
                .await;
            match result {
                Ok(o) if o.status.success() => {
                    let size = std::fs::metadata(&wav).map(|m| m.len()).unwrap_or(0);
                    ToolOutput {
                        success: true,
                        content: format!("TTS (flite)\nOutput: {}\nSize: {} bytes", wav, size),
                        error: None,
                    }
                }
                Ok(o) => ToolOutput::error(&format!(
                    "flite failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                )),
                Err(e) => ToolOutput::error(&format!("flite error: {}", e)),
            }
        }
        TtsEngine::MacSay => {
            let aiff = format!("{}.aiff", output_path.trim_end_matches(".mp3"));
            let result = tokio::process::Command::new("say")
                .args(["-v", voice, "-o", &aiff, "--", text])
                .output()
                .await;
            match result {
                Ok(o) if o.status.success() => {
                    let size = std::fs::metadata(&aiff).map(|m| m.len()).unwrap_or(0);
                    ToolOutput {
                        success: true,
                        content: format!(
                            "TTS (macOS say, {})\nOutput: {}\nSize: {} bytes",
                            voice, aiff, size
                        ),
                        error: None,
                    }
                }
                Ok(o) => ToolOutput::error(&format!(
                    "say failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                )),
                Err(e) => ToolOutput::error(&format!("say error: {}", e)),
            }
        }
        TtsEngine::None => ToolOutput {
            success: false,
            content: "No TTS engine found. Options:\n\
                      - Edge TTS (free, cloud): auto-enabled\n\
                      - OpenAI TTS: set OPENAI_API_KEY\n\
                      - Linux: sudo apt install espeak-ng\n\
                      - macOS: built-in (say command)"
                .into(),
            error: Some("No TTS engine available".into()),
        },
    }
}

#[derive(Debug)]
enum TtsEngine {
    EspeakNg,
    Flite,
    MacSay,
    None,
}

fn detect_tts_engine() -> TtsEngine {
    if std::process::Command::new("which")
        .arg("espeak-ng")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return TtsEngine::EspeakNg;
    }
    if std::process::Command::new("which")
        .arg("flite")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return TtsEngine::Flite;
    }
    if std::process::Command::new("which")
        .arg("say")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return TtsEngine::MacSay;
    }
    TtsEngine::None
}
