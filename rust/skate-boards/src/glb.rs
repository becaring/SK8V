//! Just enough of glTF's binary container to read the skinned meshes the
//! Skate 3 converter writes: accessors, skins and embedded PNG images.

use serde_json::{Map, Value};

pub struct Glb<'a> {
    pub json: Value,
    blob: &'a [u8],
}

pub fn index(value: &Value) -> Result<usize, String> {
    value
        .as_u64()
        .map(|v| v as usize)
        .ok_or_else(|| format!("expected an index, found {value}"))
}

impl<'a> Glb<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        let word = |at: usize| -> Result<usize, String> {
            bytes
                .get(at..at + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
                .ok_or_else(|| "truncated GLB".to_owned())
        };
        if bytes.get(..4) != Some(b"glTF") {
            return Err("not a GLB file".into());
        }
        let json_len = word(12)?;
        let json_bytes = bytes.get(20..20 + json_len).ok_or("truncated GLB JSON")?;
        let json = serde_json::from_slice(json_bytes).map_err(|error| error.to_string())?;
        let blob = if bytes.len() > 20 + json_len {
            let blob_len = word(20 + json_len)?;
            let blob_start = 28 + json_len;
            bytes
                .get(blob_start..blob_start + blob_len)
                .ok_or("truncated GLB binary chunk")?
        } else {
            &[]
        };
        Ok(Self { json, blob })
    }

    fn view(&self, view: usize) -> Result<(&'a [u8], Option<usize>), String> {
        let view = &self.json["bufferViews"][view];
        let offset = view["byteOffset"].as_u64().unwrap_or(0) as usize;
        let length = index(&view["byteLength"])?;
        let bytes = self
            .blob
            .get(offset..offset + length)
            .ok_or("buffer view outside the GLB")?;
        Ok((bytes, view["byteStride"].as_u64().map(|s| s as usize)))
    }

    /// Every component of an accessor, widened to `f64`.
    pub fn components(&self, accessor: usize) -> Result<Vec<f64>, String> {
        let accessor: &Map<String, Value> = self.json["accessors"][accessor]
            .as_object()
            .ok_or("missing accessor")?;
        let count = index(&accessor["count"])?;
        let width = match accessor["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            Some("MAT4") => 16,
            other => return Err(format!("unsupported accessor type {other:?}")),
        };
        let (size, read): (usize, fn(&[u8]) -> f64) = match accessor["componentType"].as_u64() {
            Some(5121) => (1, |b| f64::from(b[0])),
            Some(5123) => (2, |b| f64::from(u16::from_le_bytes([b[0], b[1]]))),
            Some(5125) => (4, |b| f64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))),
            Some(5126) => (4, |b| f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))),
            other => return Err(format!("unsupported component type {other:?}")),
        };
        let (bytes, stride) = self.view(index(&accessor["bufferView"])?)?;
        let start = accessor.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let stride = stride.unwrap_or(size * width);
        let mut out = Vec::with_capacity(count * width);
        for element in 0..count {
            for component in 0..width {
                let at = start + element * stride + component * size;
                let field = bytes.get(at..at + size).ok_or("accessor outside its view")?;
                out.push(read(field));
            }
        }
        Ok(out)
    }

    pub fn floats(&self, accessor: usize) -> Result<Vec<f32>, String> {
        Ok(self.components(accessor)?.into_iter().map(|v| v as f32).collect())
    }

    pub fn integers(&self, accessor: usize) -> Result<Vec<u32>, String> {
        Ok(self.components(accessor)?.into_iter().map(|v| v as u32).collect())
    }

    /// The bytes of an image stored in the binary chunk.
    pub fn image_bytes(&self, image: usize) -> Result<&'a [u8], String> {
        let view = index(&self.json["images"][image]["bufferView"])?;
        Ok(self.view(view)?.0)
    }
}

#[cfg(test)]
pub(crate) mod build {
    //! Writes small GLBs for the tests.
    use serde_json::{Value, json};

    #[derive(Default)]
    pub struct Writer {
        pub blob: Vec<u8>,
        pub views: Vec<Value>,
        pub accessors: Vec<Value>,
    }

    impl Writer {
        fn push(&mut self, bytes: &[u8]) -> usize {
            while self.blob.len() % 4 != 0 {
                self.blob.push(0);
            }
            let offset = self.blob.len();
            self.blob.extend_from_slice(bytes);
            self.views.push(json!({"buffer": 0, "byteOffset": offset, "byteLength": bytes.len()}));
            self.views.len() - 1
        }

        pub fn floats(&mut self, values: &[f32], kind: &str) -> usize {
            let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
            let width = match kind {
                "SCALAR" => 1,
                "VEC2" => 2,
                "VEC3" => 3,
                "VEC4" => 4,
                _ => 16,
            };
            let view = self.push(&bytes);
            self.accessors.push(json!({"bufferView": view, "componentType": 5126,
                "count": values.len() / width, "type": kind}));
            self.accessors.len() - 1
        }

        pub fn shorts(&mut self, values: &[u16], kind: &str) -> usize {
            let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
            let width = if kind == "VEC4" { 4 } else { 1 };
            let view = self.push(&bytes);
            self.accessors.push(json!({"bufferView": view, "componentType": 5123,
                "count": values.len() / width, "type": kind}));
            self.accessors.len() - 1
        }

        pub fn ints(&mut self, values: &[u32]) -> usize {
            let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
            let view = self.push(&bytes);
            self.accessors.push(json!({"bufferView": view, "componentType": 5125,
                "count": values.len(), "type": "SCALAR"}));
            self.accessors.len() - 1
        }

        pub fn image(&mut self, png: &[u8]) -> usize {
            self.push(png)
        }

        pub fn finish(mut self, mut document: Value) -> Vec<u8> {
            while self.blob.len() % 4 != 0 {
                self.blob.push(0);
            }
            document["buffers"] = json!([{"byteLength": self.blob.len()}]);
            document["bufferViews"] = Value::Array(self.views);
            document["accessors"] = Value::Array(self.accessors);
            let mut text = serde_json::to_vec(&document).unwrap();
            while text.len() % 4 != 0 {
                text.push(b' ');
            }
            let total = 12 + 8 + text.len() + 8 + self.blob.len();
            let mut out = Vec::with_capacity(total);
            out.extend_from_slice(b"glTF");
            out.extend_from_slice(&2u32.to_le_bytes());
            out.extend_from_slice(&(total as u32).to_le_bytes());
            out.extend_from_slice(&(text.len() as u32).to_le_bytes());
            out.extend_from_slice(b"JSON");
            out.extend_from_slice(&text);
            out.extend_from_slice(&(self.blob.len() as u32).to_le_bytes());
            out.extend_from_slice(b"BIN\0");
            out.extend_from_slice(&self.blob);
            out
        }
    }
}
