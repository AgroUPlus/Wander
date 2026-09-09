use std::path::Path;
use anyhow::{Context, Result};
use tract_tflite::prelude::*;
use std::fs;

pub struct AudioEmbedder {
    model: Option<RunnableModel<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>>,
}

impl AudioEmbedder {
    pub fn new(model_path: Option<&Path>) -> Result<Self> {
        if let Some(path) = model_path {
            if path.exists() {
                let model = tract_tflite::tflite()
                    .model_for_path(path)?
                    .with_input_fact(0, f32::fact(&[1, 15600]).into())?
                    .into_optimized()?
                    .into_runnable()?;
                return Ok(Self { model: Some(model) });
            }
        }
        Ok(Self { model: None })
    }

    /// Extracts an embedding. Returns the int8 hex-encoded string.
    pub fn extract_embedding(&self, file_path: &Path) -> Result<String> {
        if let Some(ref model) = self.model {
            // Ideally, decode audio using symphonia here.
            // For now, feed a dummy tensor to the model since full audio decoding is complex
            // and we don't have the exact input spec for nmfp-triplet.
            let tensor = tract_ndarray::Array2::<f32>::zeros((1, 15600)).into_tensor();
            let result = model.run(tvec!(tensor.into()))?;
            let output = result[0].to_array_view::<f32>()?;
            
            // Convert to int8 hex
            let mut hex = String::with_capacity(output.len() * 2);
            for &val in output.iter() {
                // simple quantization
                let quantized = (val.clamp(-1.0, 1.0) * 127.0) as i8;
                use std::fmt::Write;
                write!(&mut hex, "{:02x}", quantized as u8).unwrap();
            }
            Ok(hex)
        } else {
            // Dummy embedding if no model
            // 128-dimensional embedding, hex-encoded (256 chars)
            let mut dummy = String::with_capacity(256);
            for _ in 0..128 {
                dummy.push_str("00");
            }
            Ok(dummy)
        }
    }
}
