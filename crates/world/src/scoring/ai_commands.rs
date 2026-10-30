// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Scoped native AI commands. The event owner resolves manager links and assigns
//! replicated goals; graph instances retain their own command/context lifetimes.
use super::{Error, ai_control::Inclusion, participant_properties::Property};

pub const LOCATION_TRANSFORM: u32 = 0x0793_64b4;
pub const COMPLETED: u32 = 0x4bbe_1e51;
pub const ENTERED: u32 = 0xbeec_f8a5;
pub const NEXT: u32 = 0xb511_f991;
pub const MAX_COMMANDS: usize = 128;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Enable<T> {
    pub inclusion: Inclusion,
    pub add_on_enable: bool,
    pub remove_on_disable: bool,
    pub state: Option<T>,
    pub actor_manager: Property,
    pub ai_manager: Property,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Control<T> {
    pub context_hash: u32,
    pub one_by_one: bool,
    pub apply_existing: bool,
    pub states: Vec<T>,
    pub commands: Vec<T>,
    pub command_manager: Property,
    pub actor_manager: Property,
    pub context_manager: Property,
}
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Scripted {
    pub throttle: f32,
    pub steering: f32,
    pub brake: f32,
    pub handbrake: f32,
    pub use_nos: bool,
}
impl Scripted {
    pub fn validate(self) -> Result<(), Error> {
        for v in [self.throttle, self.steering, self.brake, self.handbrake] {
            super::finite(v)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Goto {
    pub location: Option<Property>,
    pub transform: [[f32; 3]; 4],
    pub drift: bool,
    pub stop: bool,
    pub radius: f32,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Definition<T> {
    State {
        handles_marked_for_delete: bool,
        send_rpc: bool,
    },
    Enable(Enable<T>),
    Control(Control<T>),
    Goto(Goto),
    Scripted(Scripted),
}
impl<T> Definition<T> {
    pub fn try_map<U, E>(self, mut map: impl FnMut(T) -> Result<U, E>) -> Result<Definition<U>, E> {
        Ok(match self {
            Self::State {
                handles_marked_for_delete,
                send_rpc,
            } => Definition::State {
                handles_marked_for_delete,
                send_rpc,
            },
            Self::Enable(v) => Definition::Enable(Enable {
                state: v.state.map(&mut map).transpose()?,
                inclusion: v.inclusion,
                add_on_enable: v.add_on_enable,
                remove_on_disable: v.remove_on_disable,
                actor_manager: v.actor_manager,
                ai_manager: v.ai_manager,
            }),
            Self::Control(v) => Definition::Control(Control {
                states: v
                    .states
                    .into_iter()
                    .map(&mut map)
                    .collect::<Result<_, _>>()?,
                commands: v
                    .commands
                    .into_iter()
                    .map(&mut map)
                    .collect::<Result<_, _>>()?,
                context_hash: v.context_hash,
                one_by_one: v.one_by_one,
                apply_existing: v.apply_existing,
                command_manager: v.command_manager,
                actor_manager: v.actor_manager,
                context_manager: v.context_manager,
            }),
            Self::Goto(v) => Definition::Goto(v),
            Self::Scripted(v) => Definition::Scripted(v),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Goto {
        point: [f32; 3],
        drift: bool,
        stop: bool,
        radius: f32,
    },
    Scripted(Scripted),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Goal {
    /// Monotonic within this graph instance. The host also retains its instance ID.
    pub token: u64,
    pub node: u16,
    pub command: Command,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Override {
    Acquire,
    Set(bool),
    Release,
}

/// Ordered effects committed with the graph transaction. Tokens must be scoped
/// by the spawning logic's instance ID when applied to shared actor managers.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Override {
        manager: Property,
        node: u16,
        participant: u64,
        action: Override,
    },
    State {
        node: u16,
        participant: u64,
        entered: bool,
    },
    Start {
        node: u16,
        token: u64,
        participant: u64,
        context_hash: u32,
        command_manager: Property,
        actor_manager: Property,
        context_manager: Property,
        goals: Vec<Goal>,
    },
    Release {
        node: u16,
        token: u64,
        participant: u64,
    },
}
