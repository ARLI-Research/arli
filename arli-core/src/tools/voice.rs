//! Voice tool — text-to-speech and speech-to-text.
//!
//! TTS: converts text to audio file using system TTS engines (espeak-ng, flite, say).
//! STT: placeholder — suggests using external services.

use async_trait::async_trait;
use super::{Tool, ToolOutput};

pub struct VoiceTool;

#[async_trait]
impl Tool for VoiceTool {
    fn name(&self) -> &str { "voice" }

    fn description(&self) -> &str {
        "Convert text to speech (TTS) or speech to text (STT). \
         TTS generates an audio file from text using system TTS engines. \
         STT converts audio to text (requires whisper or similar). \
         Actions: tts (text→audio), stt (audio→text)"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["tts", "stt"],
                    "description": "tts = text to speech, stt = speech to text",
                    "default": "tts"
                },
                "text": {
                    "type": "string",
                    "description": "Text to convert to speech (for TTS action)"
                },
                "output_path": {
                    "type": "string",
                    "description": "Path to save the output audio file (default: /tmp/arli_tts.wav)"
                },
                "voice": {
                    "type": "string",
                    "description": "Voice name for TTS (depends on engine, e.g. 'en', 'ru', 'default')",
                    "default": "en"
                },
                "input_path": {
                    "type": "string",
                    "description": "Path to audio file for STT (WAV, MP3, etc.)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => return ToolOutput::error(&format!("Invalid JSON: {}", e)),
        };

        let action = args["action"].as_str().unwrap_or("tts");

        match action {
            "tts" => execute_tts(&args).await,
            "stt" => execute_stt(&args).await,
            unknown => ToolOutput::error(&format!("Unknown action '{}'. Use: tts, stt", unknown)),
        }
    }
}

async fn execute_tts(args: &serde_json::Value) -> ToolOutput {
    let text = match args["text"].as_str() {
        Some(t) => t,
        None => return ToolOutput::error("Missing 'text' parameter for TTS"),
    };

    let output_path = args["output_path"].as_str().unwrap_or("/tmp/arli_tts.wav");
    let voice = args["voice"].as_str().unwrap_or("en");

    // Detect available TTS engine
    let engine = detect_tts_engine();

    match engine {
        TtsEngine::EspeakNg => {
            let result = tokio::process::Command::new("espeak-ng")
                .args(["-v", voice, "-w", output_path, "--", text])
                .output()
                .await;

            match result {
                Ok(output) if output.status.success() => {
                    let size = std::fs::metadata(output_path)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    ToolOutput {
                        success: true,
                        content: format!(
                            "TTS generated successfully using espeak-ng\n\
                             Voice: {}\nOutput: {}\nSize: {} bytes",
                            voice, output_path, size
                        ),
                        error: None,
                    }
                }
                Ok(output) => ToolOutput::error(&format!(
                    "espeak-ng failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )),
                Err(e) => ToolOutput::error(&format!("Failed to run espeak-ng: {}", e)),
            }
        }
        TtsEngine::Flite => {
            let wav_path = if output_path.ends_with(".wav") {
                output_path.to_string()
            } else {
                format!("{}.wav", output_path)
            };

            let result = tokio::process::Command::new("flite")
                .args(["-t", text, "-o", &wav_path])
                .output()
                .await;

            match result {
                Ok(output) if output.status.success() => {
                    let size = std::fs::metadata(&wav_path)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    ToolOutput {
                        success: true,
                        content: format!(
                            "TTS generated successfully using flite\n\
                             Output: {}\nSize: {} bytes",
                            wav_path, size
                        ),
                        error: None,
                    }
                }
                Ok(output) => ToolOutput::error(&format!(
                    "flite failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )),
                Err(e) => ToolOutput::error(&format!("Failed to run flite: {}", e)),
            }
        }
        TtsEngine::MacSay => {
            let aiff_path = if output_path.ends_with(".aiff") {
                output_path.to_string()
            } else {
                format!("{}.aiff", output_path)
            };

            let result = tokio::process::Command::new("say")
                .args(["-v", voice, "-o", &aiff_path, "--", text])
                .output()
                .await;

            match result {
                Ok(output) if output.status.success() => {
                    let size = std::fs::metadata(&aiff_path)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    ToolOutput {
                        success: true,
                        content: format!(
                            "TTS generated successfully using macOS say\n\
                             Voice: {}\nOutput: {}\nSize: {} bytes",
                            voice, aiff_path, size
                        ),
                        error: None,
                    }
                }
                Ok(output) => ToolOutput::error(&format!(
                    "say command failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )),
                Err(e) => ToolOutput::error(&format!("Failed to run say: {}", e)),
            }
        }
        TtsEngine::None => {
            ToolOutput {
                success: false,
                content: "No TTS engine found. Install one:\n\
                          \n  Linux: sudo apt install espeak-ng\n\
                          \n  macOS: built-in (say command)\n\
                          \n  Or use an online TTS API service".into(),
                error: Some("No TTS engine available".into()),
            }
        }
    }
}

async fn execute_stt(args: &serde_json::Value) -> ToolOutput {
    let input_path = match args["input_path"].as_str() {
        Some(p) => p,
        None => return ToolOutput::error("Missing 'input_path' parameter for STT"),
    };

    if !std::path::Path::new(input_path).exists() {
        return ToolOutput::error(&format!("Audio file not found: {}", input_path));
    }

    // Try whisper CLI if available
    let result = tokio::process::Command::new("whisper")
        .args([input_path, "--model", "tiny", "--output_format", "txt"])
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            // Whisper creates <input>.txt with the transcription
            let txt_path = format!("{}.txt", input_path.trim_end_matches(|c| c == '.'));
            match std::fs::read_to_string(&txt_path) {
                Ok(transcript) => ToolOutput {
                    success: true,
                    content: format!("Transcription:\n\n{}", transcript.trim()),
                    error: None,
                },
                Err(_) => ToolOutput {
                    success: true,
                    content: format!(
                        "Whisper completed but couldn't read output.\nStdout: {}",
                        String::from_utf8_lossy(&output.stdout)
                    ),
                    error: None,
                },
            }
        }
        _ => {
            ToolOutput {
                success: false,
                content: "Speech-to-text requires Whisper or similar.\n\
                          \n  Install: pip install openai-whisper\n\
                          \n  Or use: whisper <audio_file> --model tiny\n\
                          \n  Cloud API: OpenAI Whisper API, Google STT, etc.".into(),
                error: Some("Whisper not found. Install with: pip install openai-whisper".into()),
            }
        }
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
    // Check espeak-ng (Linux)
    if std::process::Command::new("which")
        .arg("espeak-ng")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return TtsEngine::EspeakNg;
    }

    // Check flite (Linux)
    if std::process::Command::new("which")
        .arg("flite")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return TtsEngine::Flite;
    }

    // Check say (macOS)
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
