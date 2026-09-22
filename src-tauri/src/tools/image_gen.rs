use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::providers::{
    Provider, ProviderConfig, ProviderError, ProviderEvent, ProviderMessage, ProviderRequest,
};

#[derive(Debug, Clone)]
struct ImageGenTool;

impl ImageGenTool {
    pub fn name() -> &'static str {
        "image_gen"
    }

    pub fn definition() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": Self::name(),
                "description": "Generate or edit images using the configured provider image endpoint.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "prompt": {
                            "type": "string",
                            "description": "Prompt for image generation or editing."
                        },
                        "model": {
                            "type": "string",
                            "description": "Optional model override for this request."
                        },
                        "steps": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Optional generation steps."
                        },
                        "seed": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "Optional random seed."
                        },
                        "cfg_scale": {
                            "type": "number",
                            "description": "Optional prompt adherence scale."
                        },
                        "response_format": {
                            "type": "string",
                            "enum": ["b64_json", "url"],
                            "description": "Preferred result format."
                        },
                        "text_mode": {
                            "type": "boolean",
                            "description": "Use text-image mode when supported."
                        },
                        "image": {
                            "type": "string",
                            "format": "uri",
                            "description": "Source image data URL for editing requests."
                        },
                        "mask": {
                            "type": "string",
                            "format": "uri",
                            "description": "Mask image data URL for editing requests."
                        }
                    },
                    "required": ["prompt"]
                }
            }
        })
    }
}
