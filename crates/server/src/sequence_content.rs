// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Failure;
use nfs_content::sequences::{ParseError, SequenceContent as Data};
use nfs_world::{arrival::Arrivals, sequences::Sequences};
use serde_json::Value;
use std::{ops::Deref, path::Path};
pub mod authored;
pub mod execution;
#[derive(Clone, Debug)]
pub struct SequenceContent(Data);
impl Deref for SequenceContent {
    type Target = Data;
    fn deref(&self) -> &Data {
        &self.0
    }
}
fn error(e: ParseError) -> Failure {
    match e {
        ParseError::Invalid => Failure::ProfileConfig,
        ParseError::Limit => Failure::BodyLimit,
        ParseError::Io => Failure::Output,
    }
}
impl SequenceContent {
    pub fn load(path: &Path) -> Result<Self, Failure> {
        Data::load(path).map(Self).map_err(error)
    }
    pub fn from_json(value: &Value) -> Result<Self, Failure> {
        Data::from_json(value).map(Self).map_err(error)
    }
    pub fn instance(&self) -> Result<Sequences, nfs_world::replication::Error> {
        Sequences::new(self.definition, self.content.clone())
    }
    pub fn arrivals(&self) -> Option<Result<Arrivals, nfs_world::replication::Error>> {
        self.arrive.as_ref().map(|(definition, content)| {
            let arrivals = Arrivals::new(*definition, content.clone())?;
            match &self.spline {
                Some(spline) => arrivals.with_spline(spline.clone()),
                None => Ok(arrivals),
            }
        })
    }
    pub fn returns(&self) -> Option<Result<Arrivals, nfs_world::replication::Error>> {
        self.returning.as_ref().map(|r| {
            Arrivals::returning(r.definition, r.content.clone(), r.duration_ms)?
                .with_spline(r.spline.clone())
        })
    }
    pub fn crashes(
        &self,
    ) -> Option<Result<nfs_world::crash::Crashes, nfs_world::replication::Error>> {
        self.crash
            .as_ref()
            .map(|c| nfs_world::crash::Crashes::new(c.definition, c.content.clone(), c.duration_ms))
    }
}
