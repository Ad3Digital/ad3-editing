// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Literal-only source writes for controls, addressed by module and binding.
use crate::{Result, evaluator::LiveProject, inspect::Control, source::SourceDocument};
use serde_json::Value;
use std::path::Path;

impl SourceDocument {
    pub fn controls(&self) -> Result<Vec<Control>> {
        if !self.source.contains("@inspect")
            && !self
                .module_sources
                .values()
                .any(|source| source.contains("@inspect"))
        {
            return Ok(Vec::new());
        }
        let mut live = LiveProject::mount_frozen(
            &self.path,
            &self.source,
            false,
            (!self.module_sources.is_empty()).then(|| self.module_sources.clone()),
        )?;
        live.inspect_controls()
    }

    pub fn set_control(&self, file: &Path, name: &str, value: Value) -> Result<Self> {
        let target =
            crate::atomic_file::source_target(&self.project_root, &self.project_root.join(file))?;
        let original = if target == self.path {
            &self.source
        } else {
            self.module_sources
                .get(&target)
                .ok_or("Control module is not in the opened project")?
        };
        let binding = crate::inspect::bindings(&target, original)?
            .into_iter()
            .find(|binding| binding.declaration.name == name)
            .ok_or("@inspect binding not found in source")?;
        binding.declaration.validate(&value)?;
        let mut updated = original.clone();
        updated.replace_range(binding.initializer, &value.to_string());
        let mut sources = self.module_sources.clone();
        if sources.is_empty() {
            sources.insert(self.path.clone(), self.source.clone());
        }
        sources.insert(target.clone(), updated.clone());
        let source = if target == self.path {
            updated
        } else {
            self.source.clone()
        };
        let mut live = LiveProject::mount_frozen(&self.path, &source, false, Some(sources))?;
        let root = live.sample(0.0, false)?;
        Ok(Self {
            path: self.path.clone(),
            project_root: self.project_root.clone(),
            source,
            root,
            module_sources: live.sources(),
        })
    }
}
