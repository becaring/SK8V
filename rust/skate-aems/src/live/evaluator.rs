//! MXB parameter graph, independently represented as typed definitions and
//! host-owned state. There are no guest pointers or translated instructions.

use super::mxb::{MixMap, Type};
use crate::glue::Params;
use std::collections::BTreeMap;
use std::path::Path;

const INSTANCE_MASK: u32 = 31 << 11;
fn with_instance(key: u32, instance: u8) -> u32 {
    (key & !INSTANCE_MASK) | ((instance as u32) << 11)
}
/// Instances per mix-map type in the live graph.
pub const LIVE_COUNTS: [u8; 14] = [1, 1, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

fn bank_key(key: u32) -> u32 {
    key & 0xe0ff_fff0
}
fn node_key(ty: u8, instance: u8, node: usize) -> u32 {
    ((ty as u32) << 16) | ((instance as u32) << 11) | node as u32
}
fn mul_q15(a: i32, b: i32) -> i32 {
    a.wrapping_mul(b) >> 15
}

#[derive(Clone, Debug)]
struct Source {
    key: u32,
    raw: i32,
    mb: i32,
}
#[derive(Clone, Debug)]
struct VariableState {
    source: usize,
    links: Vec<u32>,
    base: i32,
    cap: i32,
    mb: i32,
}
#[derive(Clone, Debug)]
struct SumState {
    sources: Vec<u32>,
    lower: i32,
    upper: i32,
    value: i32,
}
#[derive(Clone, Debug)]
struct OutputState {
    sources: Vec<u32>,
    spatial: Vec<u32>,
    base: i32,
    value: i32,
    bank: u32,
    mode: u8,
    destinations: Vec<u32>,
}

/// Positional inputs of a spatial node. The host places sounds itself, so
/// the map always sees the default: active, zero distances and angles.
#[derive(Clone, Copy, Debug)]
struct Position {
    pub distances: [f32; 2],
    pub angles: [u16; 2],
    pub velocities: [f32; 2],
    pub active: bool,
}
impl Default for Position {
    fn default() -> Self {
        Self {
            distances: [0.; 2],
            angles: [0; 2],
            velocities: [0.; 2],
            active: true,
        }
    }
}
#[derive(Clone, Debug)]
struct SpatialState {
    definition: [u32; 7],
    raw: i32,
    mb: i32,
    angle: i32,
    pitch: i32,
    previous_distance: f32,
    distance_change: f32,
}

/// Runtime instance of an MXB graph. Definitions are immutable after loading;
/// all writable state is owned here, indexed by semantic IDs rather than guest
/// allocations. Player-only construction retains global type 0 and player 1.
pub struct Evaluator {
    transfer: Transfer,
    sources: Vec<Source>,
    source_ids: BTreeMap<u32, usize>,
    variables: BTreeMap<u32, VariableState>,
    sums: BTreeMap<u32, SumState>,
    outputs: BTreeMap<u32, OutputState>,
    spatial: BTreeMap<u32, SpatialState>,
    inputs: BTreeMap<u32, [i32; 16]>,
    parameters: BTreeMap<u32, [u32; 16]>,
    active: BTreeMap<u32, bool>,
    control_definitions: BTreeMap<u32, super::mxb::Control>,
    control_values: BTreeMap<u32, (i32, i32)>,
    envelopes: BTreeMap<u32, super::evaluator_controls::Envelope>,
    /// The nodes' keys in map order, fixed once loaded, so a tick can walk a
    /// map while it reads the rest of the graph.
    variable_keys: Vec<u32>,
    control_keys: Vec<u32>,
    sum_keys: Vec<u32>,
    output_keys: Vec<u32>,
}

impl Evaluator {
    /// Rearm an existing map between skating sessions without touching disk.
    /// Restores the same writable state as a fresh, active graph.
    pub fn reset(&mut self) {
        self.reset_values();
        // The collision requests start idle (component constructor).
        for i in 0..LIVE_COUNTS[super::collisions::MIX_TYPE] as usize {
            self.set_active(super::collisions::mix_id(i), false);
        }
    }
    fn reset_values(&mut self) {
        for source in &mut self.sources {
            source.raw = 32767;
            source.mb = 0;
        }
        for variable in self.variables.values_mut() {
            variable.mb = 0;
        }
        for sum in self.sums.values_mut() {
            sum.value = 0;
        }
        for output in self.outputs.values_mut() {
            output.value = 0;
        }
        for spatial in self.spatial.values_mut() {
            spatial.raw = 32767;
            spatial.mb = 0;
            spatial.angle = 0;
            spatial.pitch = 0;
            spatial.previous_distance = 1.;
            spatial.distance_change = 1.;
        }
        for input in self.inputs.values_mut() {
            *input = [0; 16];
        }
        for parameters in self.parameters.values_mut() {
            *parameters = [0; 16];
            parameters[15] = 1;
        }
        for active in self.active.values_mut() {
            *active = true;
        }
        for value in self.control_values.values_mut() {
            *value = (0, 0);
        }
        for envelope in self.envelopes.values_mut() {
            *envelope = Default::default();
        }
    }
    /// The live graph: the global and player instances plus the retail
    /// instance counts of the ported components (type 3: the ten world
    /// collision requests, `super::collisions`).
    pub fn load(cache: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(cache.join("MixMapSK8.mxb"))
            .map_err(|e| format!("audio MixMapSK8.mxb: {e}"))?;
        let mut graph = Self::from_mxb_with_counts(&bytes, &LIVE_COUNTS)?;
        graph.reset();
        Ok(graph)
    }
    pub fn from_mxb_with_counts(bytes: &[u8], counts: &[u8]) -> Result<Self, String> {
        let map = MixMap::parse(bytes)?;
        if counts.len() != map.types.len() || counts.iter().any(|n| *n > 32) {
            return Err("MXB instance counts do not match the type directory".into());
        }
        let mut graph = Self {
            transfer: Transfer::default(),
            sources: Vec::new(),
            source_ids: BTreeMap::new(),
            variables: BTreeMap::new(),
            sums: BTreeMap::new(),
            outputs: BTreeMap::new(),
            spatial: BTreeMap::new(),
            inputs: BTreeMap::new(),
            parameters: BTreeMap::new(),
            active: BTreeMap::new(),
            control_definitions: BTreeMap::new(),
            control_values: BTreeMap::new(),
            envelopes: BTreeMap::new(),
            variable_keys: Vec::new(),
            control_keys: Vec::new(),
            sum_keys: Vec::new(),
            output_keys: Vec::new(),
        };
        for (ty, def) in map.types.iter().enumerate() {
            if let Some(def) = def {
                for instance in 0..counts[ty] {
                    graph.add_instance(ty as u8, instance, def, counts)?;
                }
            } else if counts[ty] != 0 {
                return Err("MXB instance requested for absent type".into());
            }
        }
        // The native loader groups transformed sources by curve. This is an
        // observable order when a source reads another variable's raw value.
        let mut ordering: Vec<_> = (0..graph.sources.len()).collect();
        ordering.sort_by_key(|i| (graph.sources[*i].key >> 24) & 15);
        let remap: BTreeMap<usize, usize> = ordering
            .iter()
            .enumerate()
            .map(|(new, old)| (*old, new))
            .collect();
        graph.sources = ordering.iter().map(|i| graph.sources[*i].clone()).collect();
        graph.source_ids = graph
            .sources
            .iter()
            .enumerate()
            .map(|(i, s)| (s.key, i))
            .collect();
        for v in graph.variables.values_mut() {
            v.source = remap[&v.source];
        }
        graph.variable_keys = graph.variables.keys().copied().collect();
        graph.control_keys = graph.control_definitions.keys().copied().collect();
        graph.sum_keys = graph.sums.keys().copied().collect();
        graph.output_keys = graph.outputs.keys().copied().collect();
        Ok(graph)
    }
    fn expand(key: u32, owner: u8, instance: u8, counts: &[u8]) -> Result<Vec<u32>, String> {
        let ty = ((key >> 16) & 255) as usize;
        let count = *counts.get(ty).ok_or("MXB reference has invalid type")?;
        Ok(if ty == owner as usize {
            vec![with_instance(key, instance)]
        } else {
            (0..count).map(|i| with_instance(key, i)).collect()
        })
    }
    fn add_source(&mut self, key: u32) -> usize {
        if let Some(id) = self.source_ids.get(&key) {
            return *id;
        }
        let id = self.sources.len();
        self.sources.push(Source {
            key,
            raw: 32767,
            mb: 0,
        });
        self.source_ids.insert(key, id);
        if matches!(key >> 29, 2 | 3) {
            self.inputs.entry(bank_key(key)).or_insert([0; 16]);
        }
        id
    }
    fn add_instance(
        &mut self,
        ty: u8,
        instance: u8,
        def: &Type,
        counts: &[u8],
    ) -> Result<(), String> {
        for (i, var) in def.variables.iter().enumerate() {
            let source = self.add_source(with_instance(var.source, instance));
            let mut links = Vec::new();
            for key in &var.links {
                links.extend(Self::expand(*key, ty, instance, counts)?);
            }
            let value = var.value as i16 as i32;
            self.variables.insert(
                node_key(ty, instance, i),
                VariableState {
                    source,
                    links,
                    base: value.max(0),
                    cap: 32767 - self.transfer.level(-value.abs()),
                    mb: 0,
                },
            );
        }
        for (i, control) in def.controls.iter().enumerate() {
            let mut control = control.clone();
            if !matches!((control.words[0] >> 24) & 15, 1 | 3 | 4) {
                return Err("unsupported MXB control envelope mode".into());
            }
            // Map loading normalizes zero attack/release lengths to one frame
            // before the duration decoder sees them (829528A0).
            if (control.words[0] >> 24) & 15 == 1 {
                for word in [3, 5] {
                    if control.words[word] & 0xfff == 0 {
                        control.words[word] |= 1;
                    }
                }
            }
            control.words[2] = with_instance(control.words[2], instance);
            if matches!(control.words[2] >> 29, 2 | 3) {
                self.inputs
                    .entry(bank_key(control.words[2]))
                    .or_insert([0; 16]);
            }
            control.sources = control
                .sources
                .iter()
                .map(|k| Self::expand(*k, ty, instance, counts))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect();
            self.control_definitions
                .insert(node_key(ty, instance, i), control);
            self.control_values
                .insert(node_key(ty, instance, i), (0, 0));
            self.envelopes
                .insert(node_key(ty, instance, i), Default::default());
        }
        for (i, spatial) in def.spatial.iter().enumerate() {
            self.spatial.insert(
                node_key(ty, instance, i),
                SpatialState {
                    definition: *spatial,
                    raw: 32767,
                    mb: 0,
                    angle: 0,
                    pitch: 0,
                    previous_distance: 1.,
                    distance_change: 1.,
                },
            );
        }
        for (i, sum) in def.sums.iter().enumerate() {
            let sources = sum
                .sources
                .iter()
                .map(|k| Self::expand(*k, ty, instance, counts))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect();
            self.sums.insert(
                node_key(ty, instance, i),
                SumState {
                    sources,
                    lower: (sum.limits | 0xffff0000) as i32,
                    upper: ((sum.limits >> 16) & 32767) as i32,
                    value: 0,
                },
            );
        }
        for (i, out) in def.outputs.iter().enumerate() {
            let all = out
                .sources
                .iter()
                .map(|k| Self::expand(*k, ty, instance, counts))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            let (spatial, sources) = all.into_iter().partition(|k| *k >> 29 == 4);
            let bank = bank_key(with_instance(out.handle, instance));
            self.parameters.entry(bank).or_insert_with(|| {
                let mut p = [0; 16];
                p[15] = 1;
                p
            });
            self.active.insert(bank, true);
            let dest = def
                .destinations
                .get(i)
                .ok_or("MXB output missing destinations")?;
            self.outputs.insert(
                node_key(ty, instance, i),
                OutputState {
                    sources,
                    spatial,
                    base: (out.limits >> 16) as i16 as i32,
                    value: 0,
                    bank,
                    mode: ((dest.definition >> 24) & 15) as u8,
                    destinations: dest.entries.clone(),
                },
            );
        }
        Ok(())
    }
    pub fn set_inputs(&mut self, id: u32, inputs: [i32; 16]) {
        self.inputs.insert(bank_key(id), inputs);
    }
    pub fn inputs(&self, id: u32) -> [i32; 16] {
        self.inputs.get(&bank_key(id)).copied().unwrap_or([0; 16])
    }
    pub fn set_active(&mut self, id: u32, active: bool) {
        let id = bank_key(id);
        self.active.insert(id, active);
        if let Some(p) = self.parameters.get_mut(&id) {
            p[15] = (p[15] & !1) | u32::from(active);
        }
    }
    pub fn params(&self, id: u32) -> Params {
        Params {
            words: self.parameters.get(&bank_key(id)).copied(),
        }
    }
    #[cfg(test)]
    fn source_values(&self) -> Vec<(u32, i32, i32)> {
        self.sources.iter().map(|s| (s.key, s.raw, s.mb)).collect()
    }
    #[cfg(test)]
    fn variable_values(&self) -> Vec<(u32, i32)> {
        self.variables.iter().map(|(k, v)| (*k, v.mb)).collect()
    }
    #[cfg(test)]
    fn output_values(&self) -> Vec<(u32, i32)> {
        self.outputs.iter().map(|(k, v)| (*k, v.value)).collect()
    }
    pub fn tick(&mut self, dt: f32) -> Result<(), String> {
        if !dt.is_finite() || dt <= 0. {
            return Err("MXB tick must be finite and positive".into());
        }
        self.update_sources();
        self.update_spatial();
        self.update_controls(dt);
        self.update_outputs();
        Ok(())
    }
    fn resolve(&self, key: u32, mb: bool) -> i32 {
        let node = key & 0x00ff_f8ff;
        match key >> 29 {
            0 => self.variables.get(&node).map_or(0, |v| {
                if mb {
                    v.mb
                } else {
                    self.sources[v.source].raw
                }
            }),
            1 => {
                if key & 0x10000000 != 0 {
                    self.sums.get(&node).map_or(0, |s| s.value)
                } else {
                    self.outputs.get(&node).map_or(0, |o| o.value)
                }
            }
            2 | 3 => self
                .inputs
                .get(&bank_key(key))
                .map_or(0, |v| v[(key & 15) as usize]),
            4 => self
                .spatial
                .get(&node)
                .map_or(0, |s| if mb { s.mb } else { s.raw }),
            5 => self
                .control_values
                .get(&node)
                .map_or(0, |v| if mb { v.1 } else { v.0 }),
            _ => 0,
        }
    }
    fn update_sources(&mut self) {
        for i in 0..self.sources.len() {
            let key = self.sources[i].key;
            let raw = self
                .transfer
                .shape(self.resolve(key, false), ((key >> 24) & 15) as u8);
            self.sources[i].raw = raw;
            self.sources[i].mb = self.transfer.millibels(raw);
        }
        for k in 0..self.variable_keys.len() {
            let key = self.variable_keys[k];
            let variable = &self.variables[&key];
            let q = 32767 - mul_q15(32767 - self.sources[variable.source].raw, variable.cap);
            let mb = variable.base + self.transfer.millibels(q);
            let factor = variable
                .links
                .iter()
                .fold(32767, |v, k| mul_q15(v, self.resolve(*k, false)));
            self.variables.get_mut(&key).unwrap().mb = mul_q15(mb, factor);
        }
    }
    fn update_controls(&mut self, dt: f32) {
        for k in 0..self.control_keys.len() {
            let key = self.control_keys[k];
            let control = &self.control_definitions[&key];
            let input = self.resolve(control.words[2], false);
            let inactive = self.envelopes[&key].phase == 0 && input == 0;
            let envelope = self.envelopes.get_mut(&key).unwrap();
            envelope.update(&control.words, input, dt * 1000., &self.transfer);
            let raw = envelope.raw;
            let amplitude = control.words[0] & 512 != 0;
            let signed = control.words[1] as i16 as i32;
            let cap = 32767 - self.transfer.level(-signed.abs());
            let mut mb = if inactive {
                if amplitude {
                    -10000
                } else {
                    0
                }
            } else if amplitude {
                self.transfer.millibels(raw)
            } else if signed > 0 {
                signed + self.transfer.millibels(32767 - cap + mul_q15(raw, cap))
            } else {
                self.transfer.millibels(32767 - mul_q15(raw, cap))
            };
            if !inactive && !control.sources.is_empty() {
                let factor = control
                    .sources
                    .iter()
                    .fold(32767, |v, k| mul_q15(v, self.resolve(*k, false)));
                mb = if amplitude {
                    self.transfer
                        .millibels(mul_q15(self.transfer.level(mb), factor))
                } else {
                    mul_q15(mb, factor)
                };
            }
            self.control_values.insert(key, (raw, mb));
        }
    }
    fn update_spatial(&mut self) {
        let position = Position::default();
        for state in self.spatial.values_mut() {
            if !position.active {
                state.raw = 0;
                state.mb = -10000;
                state.angle = 0;
                state.pitch = 0;
                continue;
            }
            let d = state.definition;
            let distance_selector = ((d[1] >> 12) & 15) as usize;
            let angle_selector = ((d[1] >> 8) & 15) as usize;
            let distance = match distance_selector {
                0 => position.distances[1],
                1 => position.distances[0],
                _ => -1.,
            };
            let angle = match angle_selector {
                0 => position.angles[1],
                1 => position.angles[0],
                _ => 0,
            } as i32;
            state.angle = angle;
            let quadrant = (angle >> 14) as usize;
            let fraction = angle & 16383;
            let a = d[3 + quadrant];
            let b = d[3 + (quadrant + 1) % 4];
            let (near_a, far_a) = ((a & 32767) as f32, ((a >> 16) & 32767) as f32);
            let (near_b, far_b) = ((b & 32767) as f32, ((b >> 16) & 32767) as f32);
            if distance > far_a && distance > far_b {
                state.raw = 0;
                state.mb = -10000;
                state.pitch = 0;
                continue;
            }
            let mode = ((d[2] >> [28, 16, 24, 20][quadrant]) & 15) as u8;
            let attenuation = |near: f32, far: f32| {
                let normalized = (distance.max(near).min(far) - near) / (far - near);
                self.transfer.shape((normalized * 32767.) as i32, mode)
            };
            let left = attenuation(near_a, far_a);
            let right = if fraction == 0 {
                32767
            } else {
                attenuation(near_b, far_b)
            };
            state.raw = mul_q15(32767 - 2 * fraction, left) + mul_q15(2 * fraction, right);
            state.mb = self.transfer.millibels(state.raw);
            let speed = (d[2] & 65535) as f32;
            if speed != 0. {
                let index = if distance_selector == 1 { 0 } else { 1 };
                let denominator = position.velocities[index] + speed;
                let ratio = speed / if denominator > 0. { denominator } else { speed };
                let target = self.transfer.pitch_cents(ratio);
                if state.distance_change == 0. {
                    state.distance_change = 1.;
                }
                state.distance_change = (position.distances[index] - state.previous_distance).abs();
                state.previous_distance = position.distances[index];
                state.pitch = state
                    .pitch
                    .wrapping_sub(((target as f32 - state.pitch as f32) * -0.2) as i32);
            }
        }
    }
    fn update_outputs(&mut self) {
        for k in 0..self.sum_keys.len() {
            let key = self.sum_keys[k];
            let state = &self.sums[&key];
            let value = state
                .sources
                .iter()
                .fold(0i32, |value, source| {
                    value.wrapping_add(self.resolve(*source, true))
                })
                .clamp(state.lower, state.upper);
            self.sums.get_mut(&key).unwrap().value = value;
        }
        for k in 0..self.output_keys.len() {
            let key = self.output_keys[k];
            let state = &self.outputs[&key];
            let value = if self.active.get(&state.bank).copied().unwrap_or(false) {
                state.sources.iter().fold(state.base, |value, source| {
                    value.wrapping_add(self.resolve(*source, true))
                })
            } else {
                -10000
            };
            self.outputs.get_mut(&key).unwrap().value = value;
        }
        for state in self.outputs.values() {
            let active = self.active.get(&state.bank).copied().unwrap_or(false);
            let words = self.parameters.get_mut(&state.bank).unwrap();
            for &destination in state
                .destinations
                .iter()
                .take(if active { usize::MAX } else { 1 })
            {
                let param = ((destination >> 26) & 31) as usize;
                let positional = state
                    .spatial
                    .get(((destination >> 21) & 31) as usize)
                    .and_then(|k| self.spatial.get(&(k & 0x00fff8ff)));
                let biased = state.value.wrapping_add(destination as i16 as i32);
                let value = if !active {
                    match state.mode {
                        1 => 0,
                        2 => 25000,
                        _ => -10000,
                    }
                } else if let Some(spatial) = positional {
                    if destination & 0x80000000 != 0 {
                        spatial.angle
                    } else {
                        match state.mode {
                            0 | 4 => self.transfer.level(biased.wrapping_add(spatial.mb)),
                            1 => {
                                let p = biased.wrapping_add(spatial.pitch);
                                if p < -4800 {
                                    0
                                } else {
                                    p.min(2400)
                                }
                            }
                            2 => biased.clamp(-10000, 0),
                            _ => state.value,
                        }
                    }
                } else {
                    match state.mode {
                        0 | 4 => self.transfer.level(biased),
                        1 => biased.clamp(-4800, 2400),
                        2 => (self.transfer.pitch_ratio(biased.clamp(-10000, 0)) * 25000.) as i32,
                        _ => biased.clamp(0, 25000),
                    }
                };
                let shift = (param & 1) * 16;
                words[param / 2] =
                    (words[param / 2] & !(0xffff << shift)) | ((value as u32 & 0xffff) << shift);
            }
        }
    }
}

/// Integer transfer functions used by the map. Tables are generated from the
/// mathematical definitions and checked exhaustively against the game's
/// routines (82FDB648, 82FDBFB0, 82FDC7B0); no table asset is embedded.
pub struct Transfer {
    cosine: [i32; 513],
    log: [i32; 512],
    gain: [i32; 602],
}

impl Default for Transfer {
    fn default() -> Self {
        Self {
            cosine: std::array::from_fn(|i| {
                if i >= 511 {
                    0
                } else {
                    (32767.0 * (i as f64 * std::f64::consts::FRAC_PI_2 / 511.0).cos()) as i32
                }
            }),
            log: std::array::from_fn(|i| {
                (602.0 + 2000.0 * ((16384.0 + 32.0 * i as f64) / 32768.0).log10()) as i32
            }),
            gain: std::array::from_fn(|i| {
                if i == 0 {
                    16384
                } else {
                    (32768.0 * 10.0f64.powf((i as f64 - 602.0) / 2000.0)) as i32
                }
            }),
        }
    }
}

impl Transfer {
    /// 8294B668's ten curve shapes, evaluated in Q15 integer arithmetic.
    pub fn shape(&self, value: i32, mode: u8) -> i32 {
        let interp = |index: usize, invert: bool| {
            let (mut a, mut b) = (self.cosine[index], self.cosine[index + 1]);
            if invert {
                a = 32767 - a;
                b = 32767 - b;
            }
            if a == 0 && !invert {
                return 0;
            }
            let fraction = 1023 | ((value << 9) & 15360);
            a.wrapping_add(b.wrapping_sub(a).wrapping_mul(fraction) >> 15)
        };
        match mode {
            0 => {
                let i = value >> 6;
                if (0..511).contains(&i) {
                    interp(i as usize, false)
                } else {
                    0
                }
            }
            1 => self.shape(32767i32.wrapping_sub(value), 0),
            2 => {
                let v = self.shape(value, 0);
                v.wrapping_mul(v) >> 15
            }
            3 => self.shape(32767i32.wrapping_sub(value), 2),
            4 => {
                let i = 511 - (value >> 6);
                if (0..512).contains(&i) {
                    interp(i as usize, true)
                } else {
                    0
                }
            }
            5 => self.shape(32767i32.wrapping_sub(value), 4),
            6 => {
                let v = self.shape(value, 4);
                v.wrapping_mul(v) >> 15
            }
            7 => self.shape(32767i32.wrapping_sub(value), 6),
            8 => 32767i32.wrapping_sub(value),
            9 => value,
            _ => 0,
        }
    }

    /// Quantized logarithm, 602 millibels per binary octave.
    pub fn millibels(&self, level: i32) -> i32 {
        let octave = (level as u32).leading_zeros() as i32 - 17;
        if !(0..15).contains(&octave) {
            return -10000;
        }
        let highest = 14 - octave;
        let residual = level - (1 << highest);
        let index = if highest >= 9 {
            residual >> (highest - 9)
        } else {
            (residual << (9 - highest)) | ((1 << (9 - highest)) - 1)
        };
        self.log[index as usize] - (octave + 1) * 602
    }

    /// Inverse table lookup. The asymmetric endpoints are intentional.
    pub fn level(&self, millibels: i32) -> i32 {
        let attenuation = -millibels.clamp(-10000, 0);
        let octave = attenuation / 602;
        if octave > 15 {
            return 0;
        }
        self.gain[(601 - attenuation % 602) as usize] >> octave
    }

    /// Cents-to-frequency ratio with the map's two-stage semitone/cent
    /// quantization. Keeping the reciprocal stages separate matters at f32.
    pub fn pitch_ratio(&self, cents: i32) -> f32 {
        let magnitude = cents.unsigned_abs();
        let octaves = magnitude / 1200;
        let remainder = magnitude % 1200;
        let semitone = 2.0f64.powf((remainder / 100) as f64 / 12.0) as f32;
        let cent_index = remainder % 100;
        let cent = 2.0f64.powf(cent_index as f64 / 1200.0) as f32;
        // The source's single-precision table differs from correctly rounded
        // pow at four positions, each by one ULP (82FDCFE8).
        let cent = match cent_index {
            15 => f32::from_bits(cent.to_bits() - 1),
            36 | 56 | 65 => f32::from_bits(cent.to_bits() + 1),
            _ => cent,
        };
        let octave = 2.0f32.powi(octaves as i32);
        if cents < 0 {
            ((1.0 / semitone) / octave) * (1.0 / cent)
        } else {
            (cent * semitone) * octave
        }
    }
    /// Quantized inverse used by the positional Doppler stage (8294AF70).
    pub fn pitch_cents(&self, ratio: f32) -> i32 {
        let reciprocal = ratio > 1.;
        let q = if reciprocal {
            32767. / ratio
        } else {
            32767. * ratio
        };
        let logarithm = self.millibels(q as i32) as f32;
        (logarithm * if reciprocal { -1.9931569 } else { 1.9931569 }) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn single_gain_map() -> Vec<u8> {
        // One input variable, one output and one destination. This authored
        // fixture contains no owned-game data and exercises the full reader.
        let mut words = vec![0, 1, 16, u32::MAX, 20];
        words.extend([0, 32, u32::MAX, u32::MAX, 56, 88, u32::MAX, u32::MAX]);
        words.extend([1, 0, 0, 0, 0x49000000, (-10000i16) as u16 as u32]);
        words.extend([1, 1, 0, 0, 0xc0010000, 0x0000d8f0, 0x40000000, 0x09000000]);
        words.extend([1, (1 << 26) | (31 << 21)]);
        words.into_iter().flat_map(u32::to_be_bytes).collect()
    }
    #[test]
    fn owned_graph_input_reaches_packed_gain_and_activation() {
        let mut e = Evaluator::from_mxb_with_counts(&single_gain_map(), &[1]).unwrap();
        e.tick(1. / 60.).unwrap();
        let quiet = e.params(0x40000000).words.unwrap()[0] >> 16;
        let mut input = [0; 16];
        input[0] = 32767;
        e.set_inputs(0x40000000, input);
        e.tick(1. / 60.).unwrap();
        assert_eq!(e.variable_values(), vec![(0, -1)]);
        assert_eq!(e.params(0x40000000).words.unwrap()[0] >> 16, 32692);
        assert!(quiet < 32692);
        e.set_active(0x40000000, false);
        e.tick(1. / 60.).unwrap();
        assert_eq!(e.output_values(), vec![(0, -10000)]);
        let words = e.params(0x40000000).words.unwrap();
        assert_eq!(words[0] >> 16, 0xd8f0);
        assert_eq!(words[15] & 1, 0);
        e.reset();
        let mut fresh = Evaluator::from_mxb_with_counts(&single_gain_map(), &[1]).unwrap();
        e.tick(1. / 60.).unwrap();
        fresh.tick(1. / 60.).unwrap();
        assert_eq!(e.params(0x40000000).words, fresh.params(0x40000000).words);
        assert_eq!(e.source_values(), fresh.source_values());
        assert!(e.tick(f32::NAN).is_err());
    }
    #[test]
    fn fixed_point_endpoints_and_octaves() {
        let t = Transfer::default();
        assert_eq!(t.shape(0, 0), 32766);
        assert_eq!(t.shape(32767, 0), 0);
        assert_eq!(t.shape(32767, 9), 32767);
        assert_eq!(t.millibels(0), -10000);
        assert_eq!(t.millibels(16384), -602);
        assert_eq!(t.millibels(8192), -1204);
        assert_eq!(t.level(-10000), 0);
        assert_eq!(t.level(-602), t.level(0) >> 1);
    }
}
