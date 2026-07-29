//! Plugin registry — unified lifecycle manager for all plugin formats.

use std::path::Path;

use anyhow::{Result, bail};

use crate::plugin_types::{PluginDescriptor, PluginHostPort, PluginKind};
use crate::scanner::FileScanHost;

#[allow(dead_code)]
pub struct PluginInstance {
    pub registry_id: u64,
    pub host_id: u64,
    adapter_idx: usize,
    pub descriptor: PluginDescriptor,
    pub state: InstanceState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum InstanceState {
    Active,
    Suspended,
    Destroyed,
}

#[allow(dead_code)]
pub struct PluginRegistry {
    adapters: Vec<Box<dyn PluginHostPort>>,
    instances: Vec<PluginInstance>,
    next_id: u64,
}

#[allow(dead_code)]
impl PluginRegistry {
    pub fn new() -> Self {
        Self { adapters: Vec::new(), instances: Vec::new(), next_id: 1 }
    }

    pub fn register_adapter(&mut self, adapter: Box<dyn PluginHostPort>) {
        self.adapters.push(adapter);
    }

    pub fn with_default_adapters() -> Self {
        let mut reg = Self::new();
        for kind in [
            PluginKind::Ladspa, PluginKind::Dssi, PluginKind::Sfz,
            PluginKind::Sf2, PluginKind::Jsfx,
        ] {
            reg.register_adapter(Box::new(FileScanHost::new(kind)));
        }
        reg
    }

    pub fn scan_default_locations(&mut self, extra_dirs: &[std::path::PathBuf]) -> usize {
        let mut total = 0;
        for kind in [
            PluginKind::Ladspa, PluginKind::Dssi, PluginKind::Sfz,
            PluginKind::Sf2, PluginKind::Jsfx,
        ] {
            for dir in crate::scanner::default_search_paths(&kind) {
                total += self.scan(&dir).len();
            }
        }
        for dir in extra_dirs {
            total += self.scan(dir).len();
        }
        total
    }

    pub fn scan(&mut self, dir: &Path) -> Vec<PluginDescriptor> {
        let mut all = Vec::new();
        for adapter in &mut self.adapters {
            if let Ok(found) = adapter.scan(dir) {
                all.extend(found);
            }
        }
        all
    }

    pub fn list_plugins(&self) -> Vec<&PluginDescriptor> {
        self.adapters.iter().flat_map(|a| a.list_plugins()).collect()
    }

    pub fn find_plugin(&self, plugin_id: &str) -> Option<&PluginDescriptor> {
        self.adapters.iter().flat_map(|a| a.list_plugins()).find(|d| d.id == plugin_id)
    }

    pub fn instantiate(&mut self, plugin_id: &str, sample_rate: u32, block_size: u32) -> Result<u64> {
        let adapter_idx = self.adapters.iter()
            .position(|a| a.list_plugins().iter().any(|p| p.id == plugin_id))
            .ok_or_else(|| anyhow::anyhow!("No adapter knows plugin: {plugin_id}"))?;

        let descriptor = self.adapters[adapter_idx]
            .list_plugins().iter().find(|p| p.id == plugin_id).cloned().unwrap();

        let host_id = self.adapters[adapter_idx].instantiate(plugin_id, sample_rate, block_size)?;

        let registry_id = self.next_id;
        self.next_id += 1;

        self.instances.push(PluginInstance {
            registry_id, host_id, adapter_idx, descriptor,
            state: InstanceState::Active,
        });

        Ok(registry_id)
    }

    pub fn process(&mut self, registry_id: u64, input: &[f32], output: &mut [f32]) -> Result<()> {
        let inst = self.instances.iter()
            .find(|i| i.registry_id == registry_id)
            .ok_or_else(|| anyhow::anyhow!("Instance {registry_id} not found"))?;

        if inst.state != InstanceState::Active {
            bail!("Instance {registry_id} is not active");
        }

        let (adapter_idx, host_id) = (inst.adapter_idx, inst.host_id);
        self.adapters[adapter_idx].process(host_id, input, output)
    }

    pub fn destroy(&mut self, registry_id: u64) {
        if let Some(idx) = self.instances.iter().position(|i| i.registry_id == registry_id) {
            let inst = &mut self.instances[idx];
            if inst.state != InstanceState::Destroyed {
                self.adapters[inst.adapter_idx].destroy(inst.host_id);
                inst.state = InstanceState::Destroyed;
            }
            self.instances.swap_remove(idx);
        }
    }

    pub fn param_count(&self, registry_id: u64) -> u32 {
        if let Some(inst) = self.instances.iter().find(|i| i.registry_id == registry_id) {
            return self.adapters[inst.adapter_idx].param_count(inst.host_id);
        }
        0
    }

    pub fn get_param(&self, registry_id: u64, param_id: u32) -> f32 {
        if let Some(inst) = self.instances.iter().find(|i| i.registry_id == registry_id) {
            return self.adapters[inst.adapter_idx].get_param(inst.host_id, param_id);
        }
        0.0
    }

    pub fn set_param(&mut self, registry_id: u64, param_id: u32, value: f32) {
        if let Some(inst) = self.instances.iter().find(|i| i.registry_id == registry_id) {
            let (adapter_idx, host_id) = (inst.adapter_idx, inst.host_id);
            self.adapters[adapter_idx].set_param(host_id, param_id, value);
        }
    }

    pub fn param_name(&self, registry_id: u64, param_id: u32) -> String {
        if let Some(inst) = self.instances.iter().find(|i| i.registry_id == registry_id) {
            return self.adapters[inst.adapter_idx].param_name(inst.host_id, param_id);
        }
        format!("P{param_id}")
    }

    pub fn shutdown(&mut self) {
        let ids: Vec<u64> = self.instances.iter().map(|i| i.registry_id).collect();
        for id in ids { self.destroy(id); }
        self.adapters.clear();
    }
}

impl Default for PluginRegistry {
    fn default() -> Self { Self::new() }
}
