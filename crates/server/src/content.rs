// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Failure;
use nfs_world::content::{
    self, LoadLevel, Message, RegistrationDefinition, RegistrationsState, SubLevelNames,
};
use serde_json::{Value, json};
use std::{fs::File, io::Read, path::Path};

pub use nfs_services::SUPPORTED_BUILD_SHA256 as BUILD;
pub const FORMAT: &str = "nfs-world-content";
pub const MAX_STORE_BYTES: u64 = 2 * 1024 * 1024;
mod scenes;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Batch {
    Names(SubLevelNames),
    Registrations {
        header: u64,
        definitions: Vec<RegistrationDefinition>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldContent {
    pub level: LoadLevel,
    pub roles: Option<crate::scene_roles::SceneRoles>,
    pub batches: Vec<Batch>,
    pub scene_profiles: Option<nfs_world::replication::sublevel::Content>,
    pub launchers: Option<nfs_world::launchers::Catalog>,
}

fn array(v: &Value, max: usize) -> Result<&[Value], Failure> {
    let a = v.as_array().ok_or(Failure::ProfileConfig)?;
    if a.len() > max {
        return Err(Failure::BodyLimit);
    }
    Ok(a)
}
fn uint<T: TryFrom<u64>>(v: &Value) -> Result<T, Failure> {
    T::try_from(v.as_u64().ok_or(Failure::ProfileConfig)?).map_err(|_| Failure::ProfileConfig)
}
fn boolean(v: &Value) -> Result<bool, Failure> {
    v.as_bool().ok_or(Failure::ProfileConfig)
}
fn bytes(v: &Value) -> Result<Vec<u8>, Failure> {
    array(v, content::MAX_STRING)?.iter().map(uint).collect()
}
fn fields(v: &Value, names: &[&str]) -> Result<(), Failure> {
    let o = v.as_object().ok_or(Failure::ProfileConfig)?;
    if o.len() != names.len() || !names.iter().all(|k| o.contains_key(*k)) {
        return Err(Failure::ProfileConfig);
    }
    Ok(())
}
fn tuple(v: &Value, n: usize) -> Result<&[Value], Failure> {
    let a = array(v, n)?;
    if a.len() != n {
        return Err(Failure::ProfileConfig);
    }
    Ok(a)
}
fn signed(v: &Value) -> Result<i32, Failure> {
    i32::try_from(v.as_i64().ok_or(Failure::ProfileConfig)?).map_err(|_| Failure::ProfileConfig)
}

impl WorldContent {
    /// Checks deployment roles before listeners or world allocations are created.
    /// Earlier content formats remain readable by offline inspection tools.
    pub fn validate_runtime(&self) -> Result<(), Failure> {
        let roles = self.roles.as_ref().ok_or(Failure::ProfileConfig)?;
        roles.validate()?;
        if roles.level != self.level.level {
            return Err(Failure::ProfileConfig);
        }
        let profiles = self.scene_profiles.as_ref().ok_or(Failure::ProfileConfig)?;
        self.launchers.as_ref().ok_or(Failure::ProfileConfig)?;
        for key in [
            1,
            roles.gameplay,
            roles.startup,
            roles.garage,
            roles.progression,
        ]
        .into_iter()
        .chain(roles.traffic)
        .chain(roles.spawn_points)
        {
            profiles.profile(key).map_err(|_| Failure::ProfileConfig)?;
        }
        let messages = self.generate()?;
        let bindings = nfs_world::content::bindings::Bindings::from_messages(&messages)
            .map_err(|_| Failure::ProfileConfig)?;
        for key in [
            roles.gameplay,
            roles.startup,
            roles.garage,
            roles.progression,
        ]
        .into_iter()
        .chain(roles.spawn_points)
        {
            if bindings
                .hierarchy()
                .iter()
                .filter(|level| level.content_key == key)
                .count()
                != 1
            {
                return Err(Failure::ProfileConfig);
            }
        }
        for key in roles.traffic {
            bindings
                .root_child(key)
                .map_err(|_| Failure::ProfileConfig)?;
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, Failure> {
        let f = File::open(path).map_err(|_| Failure::Output)?;
        let mut raw = Vec::new();
        f.take(MAX_STORE_BYTES + 1)
            .read_to_end(&mut raw)
            .map_err(|_| Failure::Output)?;
        if raw.len() as u64 > MAX_STORE_BYTES {
            return Err(Failure::BodyLimit);
        }
        Self::from_json(&serde_json::from_slice(&raw).map_err(|_| Failure::ProfileConfig)?)
    }

    pub fn from_json(v: &Value) -> Result<Self, Failure> {
        let version = uint::<u8>(&v["version"])?;
        let names: &[&str] = match version {
            1 => &["format", "version", "build_sha256", "level", "batches"],
            2 => &[
                "format",
                "version",
                "build_sha256",
                "level",
                "batches",
                "scene_profiles",
            ],
            3 => &[
                "format",
                "version",
                "build_sha256",
                "level",
                "batches",
                "scene_profiles",
                "launchers",
            ],
            4 => &[
                "format",
                "version",
                "build_sha256",
                "level",
                "batches",
                "scene_profiles",
                "launchers",
                "roles",
            ],
            _ => return Err(Failure::ProfileConfig),
        };
        fields(v, names)?;
        if v["format"] != FORMAT || v["build_sha256"] != BUILD {
            return Err(Failure::ProfileConfig);
        }
        let l = &v["level"];
        fields(
            l,
            &[
                "level",
                "attributes",
                "word",
                "text",
                "flags",
                "entries",
                "final_word",
            ],
        )?;
        let flags = tuple(&l["flags"], 3)?;
        let level = LoadLevel {
            level: bytes(&l["level"])?,
            attributes: array(&l["attributes"], content::MAX_ITEMS)?
                .iter()
                .map(|v| {
                    let a = tuple(v, 2)?;
                    Ok((bytes(&a[0])?, bytes(&a[1])?))
                })
                .collect::<Result<_, Failure>>()?,
            word: uint(&l["word"])?,
            text: bytes(&l["text"])?,
            flags: [
                boolean(&flags[0])?,
                boolean(&flags[1])?,
                boolean(&flags[2])?,
            ],
            entries: array(&l["entries"], content::MAX_ITEMS)?
                .iter()
                .map(|v| {
                    let a = tuple(v, 3)?;
                    Ok((bytes(&a[0])?, signed(&a[1])?, signed(&a[2])?))
                })
                .collect::<Result<_, Failure>>()?,
            final_word: uint(&l["final_word"])?,
        };
        let mut batches = Vec::new();
        for b in array(&v["batches"], content::MAX_BATCHES - 1)? {
            let batch = match b["kind"].as_str() {
                Some("names") => {
                    fields(b, &["kind", "word", "entries"])?;
                    Batch::Names(SubLevelNames {
                        word: uint(&b["word"])?,
                        entries: array(&b["entries"], content::MAX_ITEMS)?
                            .iter()
                            .map(|v| {
                                let a = tuple(v, 2)?;
                                Ok((uint(&a[0])?, bytes(&a[1])?))
                            })
                            .collect::<Result<_, Failure>>()?,
                    })
                }
                Some("registrations") => {
                    fields(b, &["kind", "header", "definitions"])?;
                    let mut definitions = Vec::new();
                    for d in array(&b["definitions"], content::MAX_ITEMS)? {
                        fields(
                            d,
                            &[
                                "region", "byte", "flag", "enum3", "word2", "bit", "enum2",
                                "word3", "text",
                            ],
                        )?;
                        definitions.push(RegistrationDefinition {
                            region: uint(&d["region"])?,
                            byte: uint(&d["byte"])?,
                            flag: boolean(&d["flag"])?,
                            enum3: uint(&d["enum3"])?,
                            word2: uint(&d["word2"])?,
                            bit: boolean(&d["bit"])?,
                            enum2: uint(&d["enum2"])?,
                            word3: uint(&d["word3"])?,
                            text: bytes(&d["text"])?,
                        });
                    }
                    Batch::Registrations {
                        header: uint(&b["header"])?,
                        definitions,
                    }
                }
                _ => return Err(Failure::ProfileConfig),
            };
            batches.push(batch);
        }
        let scene_profiles = (version >= 2)
            .then(|| scenes::parse(&v["scene_profiles"]))
            .transpose()?;
        let launchers = if version >= 3 {
            Some(scenes::launchers(
                &v["launchers"],
                scene_profiles.as_ref().ok_or(Failure::ProfileConfig)?,
            )?)
        } else {
            None
        };
        let roles = (version == 4)
            .then(|| crate::scene_roles::SceneRoles::from_json(&v["roles"]))
            .transpose()?;
        let result = Self {
            roles,
            level,
            batches,
            scene_profiles,
            launchers,
        };
        result.generate()?;
        Ok(result)
    }

    pub fn to_json(&self) -> Value {
        let l = &self.level;
        let mut value = json!({"format":FORMAT,"version":1,"build_sha256":BUILD,
            "level":{"level":l.level,"attributes":l.attributes,"word":l.word,"text":l.text,"flags":l.flags,"entries":l.entries,"final_word":l.final_word},
            "batches":self.batches.iter().map(|b| match b {
                Batch::Names(n)=>json!({"kind":"names","word":n.word,"entries":n.entries}),
                Batch::Registrations{header,definitions}=>json!({"kind":"registrations","header":header,
                    "definitions":definitions.iter().map(|d|json!({"region":d.region,"byte":d.byte,"flag":d.flag,"enum3":d.enum3,"word2":d.word2,"bit":d.bit,"enum2":d.enum2,"word3":d.word3,"text":d.text})).collect::<Vec<_>>()})
            }).collect::<Vec<_>>()});
        if let Some(profiles) = &self.scene_profiles {
            value["version"] = json!(2);
            value["scene_profiles"] = scenes::json(profiles);
        }
        if let Some(launchers) = &self.launchers {
            value["version"] = json!(3);
            value["launchers"] = json!(
                launchers
                    .entries()
                    .map(|(key, indices)| json!({"content_key":key,"serializers":indices}))
                    .collect::<Vec<_>>()
            );
        }
        if let Some(roles) = &self.roles {
            value["version"] = json!(4);
            value["roles"] = roles.to_json();
        }
        value
    }
    pub fn generate(&self) -> Result<Vec<Message>, Failure> {
        if self.batches.len() >= content::MAX_BATCHES {
            return Err(Failure::BodyLimit);
        }
        let mut allocator = RegistrationsState::default();
        let mut messages = vec![Message::LoadLevel(self.level.clone())];
        for b in &self.batches {
            messages.push(match b {
                Batch::Names(n) => Message::Names(n.clone()),
                Batch::Registrations {
                    header,
                    definitions,
                } => allocator
                    .prepare(*header, definitions)
                    .map_err(|_| Failure::ProfileConfig)?,
            });
        }
        for m in &messages {
            m.encode().map_err(|_| Failure::ProfileConfig)?;
        }
        Ok(messages)
    }
    pub fn from_messages(messages: &[Message]) -> Result<Self, Failure> {
        if messages.len() > content::MAX_BATCHES {
            return Err(Failure::BodyLimit);
        }
        let Some(Message::LoadLevel(level)) = messages.first() else {
            return Err(Failure::ProfileConfig);
        };
        let mut result = Self {
            level: level.clone(),
            roles: None,
            batches: Vec::new(),
            scene_profiles: None,
            launchers: None,
        };
        for m in &messages[1..] {
            result.batches.push(match m {
                Message::Names(n) => Batch::Names(n.clone()),
                Message::Registrations(r) => Batch::Registrations {
                    header: r.header,
                    definitions: r
                        .entries
                        .iter()
                        .map(RegistrationDefinition::from_record)
                        .collect::<Result<_, _>>()
                        .map_err(|_| Failure::ProfileConfig)?,
                },
                Message::LoadLevel(_) => return Err(Failure::ProfileConfig),
            });
        }
        if result.generate()? != messages {
            return Err(Failure::ProfileConfig);
        }
        Ok(result)
    }
}
