// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

#[cfg(test)]
mod police_tests;

fn bytes<const N: usize>(r: &mut Reader<'_>, n: u8) -> Result<[u8; N], Error> {
    array(|| Ok(r.take(n)? as u8))
}
fn words<const N: usize>(r: &mut Reader<'_>, n: u8) -> Result<[u16; N], Error> {
    array(|| Ok(r.take(n)? as u16))
}
fn put_bytes(w: &mut Writer, v: &[u8], n: usize) -> Result<(), Error> {
    for x in v {
        w.put(u64::from(*x), n)?;
    }
    Ok(())
}
fn put_words(w: &mut Writer, v: &[u16], n: usize) -> Result<(), Error> {
    for x in v {
        w.put(u64::from(*x), n)?;
    }
    Ok(())
}
fn put_dwords(w: &mut Writer, v: &[u32]) -> Result<(), Error> {
    for x in v {
        w.put(u64::from(*x), 32)?;
    }
    Ok(())
}
fn rgb_read<const N: usize>(r: &mut Reader<'_>) -> Result<[[u8; 3]; N], Error> {
    array(|| bytes(r, 8))
}
fn rgb_write<const N: usize>(w: &mut Writer, v: &[[u8; 3]; N]) -> Result<(), Error> {
    for rgb in v {
        put_bytes(w, rgb, 8)?;
    }
    Ok(())
}

fn custom_read(r: &mut Reader<'_>) -> Result<Customization, Error> {
    Ok(Customization {
        value: r.take(8)? as u8,
        list_a: r.string(8, 255)?,
        list_b: r.string(8, 255)?,
        index: r.optional(|r| Ok(r.take(6)? as u8))?,
        values: words(r, 16)?,
        flag: r.bit()?,
        tuning: bytes(r, 4)?,
    })
}
fn custom_write(w: &mut Writer, c: &Customization) -> Result<(), Error> {
    w.put(u64::from(c.value), 8)?;
    w.string(&c.list_a, 8, 255)?;
    w.string(&c.list_b, 8, 255)?;
    w.optional(&c.index, |w, v| w.put(u64::from(*v), 6))?;
    put_words(w, &c.values, 16)?;
    w.bit(c.flag)?;
    put_bytes(w, &c.tuning, 4)
}
const CONTROL_WIDTHS: [u8; 11] = [4, 4, 10, 4, 2, 4, 1, 6, 4, 4, 1];
fn physics_read(r: &mut Reader<'_>) -> Result<Physics, Error> {
    let identity_words = array(|| r.take(32))?;
    movement_read(r, identity_words)
}
pub(super) fn movement_read(
    r: &mut Reader<'_>,
    identity_words: [u32; 3],
) -> Result<Physics, Error> {
    let position = Vector::read(r, 7)?;
    let rotation = Quaternion14::read(r)?;
    let velocity = SparseVector::read(r, 5)?;
    let angular_velocity = SparseVector::read(r, 8)?;
    let value3 = r.take(3)? as u8;
    let value6 = r.take(6)? as u8;
    let optional6 = r.optional(|r| Ok(r.take(6)? as u8))?;
    let flag = r.bit()?;
    let mut values = [0; 11];
    for (v, n) in values.iter_mut().zip(CONTROL_WIDTHS) {
        *v = r.take(n)? as u16;
    }
    let flags = array(|| r.bit())?;
    let wheel_pairs = if r.bit()? {
        None
    } else {
        Some(array(|| bytes(r, 8))?)
    };
    Ok(Physics {
        identity_words,
        position,
        rotation,
        velocity,
        angular_velocity,
        controls: PhysicsControls {
            value3,
            value6,
            optional6,
            flag,
            values,
            flags,
            wheel_pairs,
        },
    })
}
fn physics_write(w: &mut Writer, p: &Physics) -> Result<(), Error> {
    put_dwords(w, &p.identity_words)?;
    movement_write(w, p)
}
pub(super) fn movement_write(w: &mut Writer, p: &Physics) -> Result<(), Error> {
    p.position.write(w, 7)?;
    p.rotation.write(w)?;
    p.velocity.write(w, 5)?;
    p.angular_velocity.write(w, 8)?;
    let c = &p.controls;
    w.put(u64::from(c.value3), 3)?;
    w.put(u64::from(c.value6), 6)?;
    w.optional(&c.optional6, |w, v| w.put(u64::from(*v), 6))?;
    w.bit(c.flag)?;
    for (v, n) in c.values.iter().zip(CONTROL_WIDTHS) {
        w.put(u64::from(*v), usize::from(n))?;
    }
    for f in c.flags {
        w.bit(f)?;
    }
    w.bit(c.wheel_pairs.is_none())?;
    if let Some(pairs) = &c.wheel_pairs {
        for pair in pairs {
            put_bytes(w, pair, 8)?;
        }
    }
    Ok(())
}
fn appearance_read(r: &mut Reader<'_>) -> Result<Appearance, Error> {
    Ok(Appearance {
        paint: r.optional(|r| {
            Ok(Paint {
                colors: rgb_read(r)?,
                value: r.take(8)? as u8,
                name: r.string(10, 1023)?,
                flag: r.bit()?,
                resource: Resource::read(r)?,
            })
        })?,
        palette1: r.optional(rgb_read)?,
        palette2: r.optional(rgb_read)?,
        wrap: r.optional(|r| {
            Ok(if r.bit()? {
                Wrap::Preset(bytes(r, 8)?)
            } else {
                Wrap::Custom {
                    words: array(|| r.take(32))?,
                    resource: Resource::read(r)?,
                    flag: r.bit()?,
                    value: r.take(32)? as i32,
                }
            })
        })?,
        index: r.optional(|r| Ok(r.take(8)? as u8))?,
        indices: r.optional(|r| bytes(r, 8))?,
    })
}
fn appearance_write(w: &mut Writer, a: &Appearance) -> Result<(), Error> {
    w.optional(&a.paint, |w, p| {
        rgb_write(w, &p.colors)?;
        w.put(u64::from(p.value), 8)?;
        w.string(&p.name, 10, 1023)?;
        w.bit(p.flag)?;
        p.resource.write(w)
    })?;
    w.optional(&a.palette1, rgb_write)?;
    w.optional(&a.palette2, rgb_write)?;
    w.optional(&a.wrap, |w, wrap| {
        match wrap {
            Wrap::Preset(indices) => {
                w.bit(true)?;
                put_bytes(w, indices, 8)?;
            }
            Wrap::Custom {
                words,
                resource,
                flag,
                value,
            } => {
                w.bit(false)?;
                put_dwords(w, words)?;
                resource.write(w)?;
                w.bit(*flag)?;
                w.put(u64::from(*value as u32), 32)?;
            }
        }
        Ok(())
    })?;
    w.optional(&a.index, |w, v| w.put(u64::from(*v), 8))?;
    w.optional(&a.indices, |w, v| put_bytes(w, v, 8))
}
fn mesh_read(r: &mut Reader<'_>, profile: &[MeshKind]) -> Result<MeshCustomization, Error> {
    let mut meshes = Vec::with_capacity(profile.len());
    for k in profile {
        let index = signed(r, k.index_bits)? as i16;
        let extra = match k.extra {
            MeshExtra::None => MeshValue::None,
            MeshExtra::Light => MeshValue::Light(r.take(32)?),
            MeshExtra::Spoiler => MeshValue::Spoiler {
                normalized: r.take(16)? as u16,
                selector: r.take(8)? as u8,
            },
            MeshExtra::Rim => MeshValue::Rim {
                first: signed(r, 9)? as i16,
                middle: r.take(9)? as u16,
                last: signed(r, 9)? as i16,
            },
        };
        meshes.push(Mesh { index, extra });
    }
    let count = r.take(16)? as usize;
    if count > MAX_MESH_ASSETS {
        return Err(Error::Bound);
    }
    let assets = (0..count)
        .map(|_| Ok(r.take(16)? as u16))
        .collect::<Result<_, _>>()?;
    Ok(MeshCustomization { meshes, assets })
}
fn mesh_write(w: &mut Writer, profile: &[MeshKind], v: &MeshCustomization) -> Result<(), Error> {
    if v.meshes.len() != profile.len() {
        return Err(Error::Shape);
    }
    if v.assets.len() > MAX_MESH_ASSETS {
        return Err(Error::Bound);
    }
    for (k, m) in profile.iter().zip(&v.meshes) {
        put_signed(w, i32::from(m.index), k.index_bits)?;
        match (&m.extra, k.extra) {
            (MeshValue::None, MeshExtra::None) => (),
            (MeshValue::Light(value), MeshExtra::Light) => w.put(u64::from(*value), 32)?,
            (
                MeshValue::Spoiler {
                    normalized,
                    selector,
                },
                MeshExtra::Spoiler,
            ) => {
                w.put(u64::from(*normalized), 16)?;
                w.put(u64::from(*selector), 8)?;
            }
            (
                MeshValue::Rim {
                    first,
                    middle,
                    last,
                },
                MeshExtra::Rim,
            ) => {
                put_signed(w, i32::from(*first), 9)?;
                w.put(u64::from(*middle), 9)?;
                put_signed(w, i32::from(*last), 9)?;
            }
            _ => return Err(Error::TypeMismatch),
        }
    }
    w.put(v.assets.len() as u64, 16)?;
    put_words(w, &v.assets, 16)
}

pub(super) fn initial_read(r: &mut Reader<'_>, k: &Kind) -> Result<Initial, Error> {
    Ok(match k {
        Kind::Root { property_owner } => Initial::Root(Box::new(RootInitial {
            property_value: if *property_owner {
                Some(r.take(16)? as u16)
            } else {
                None
            },
            position: Vector::read(r, 5)?,
            rotation: Quaternion25::read(r)?,
            flag: r.bit()?,
            reference: r.optional(Reader::reference)?,
            rpc: read_rpc(r)?,
            fine_position: Vector::read(r, 10)?,
            fine_rotation: Quaternion25::read(r)?,
            fine_flag: r.bit()?,
        })),
        Kind::Chassis => Initial::Chassis(Box::new(ChassisInitial {
            rpc: read_rpc(r)?,
            vectors: array(|| sparse_float_read(r))?,
            custom: custom_read(r)?,
        })),
        Kind::Part { .. } => Initial::Part(r.bit()?),
        Kind::Bool | Kind::Mesh(_) | Kind::Empty => Initial::Empty,
        Kind::Wheel => Initial::Wheel {
            rpc: read_rpc(r)?,
            flag: r.bit()?,
        },
        Kind::SearchLight => Initial::SearchLight {
            rpc: read_rpc(r)?,
            enabled: r.bit()?,
        },
        Kind::Siren => Initial::Siren {
            rpc: read_rpc(r)?,
            flags: array(|| r.bit())?,
        },
        Kind::Index => Initial::Index {
            rpc: read_rpc(r)?,
            index: r.optional(|r| Ok(r.take(4)? as u8))?,
            value: r.take(4)? as u8,
        },
        Kind::FourBit => Initial::FourBit(r.take(4)? as u8),
        Kind::TripleNibbles
        | Kind::Tuning
        | Kind::Tagged
        | Kind::NibblesGuid
        | Kind::Appearance
        | Kind::GuidFloat
        | Kind::Rpc => Initial::Rpc(read_rpc(r)?),
    })
}
pub(super) fn initial_write(w: &mut Writer, k: &Kind, v: &Initial) -> Result<(), Error> {
    match (k, v) {
        (Kind::Root { property_owner }, Initial::Root(v)) => {
            if *property_owner != v.property_value.is_some() {
                return Err(Error::Shape);
            }
            if let Some(value) = v.property_value {
                w.put(u64::from(value), 16)?;
            }
            v.position.write(w, 5)?;
            v.rotation.write(w)?;
            w.bit(v.flag)?;
            w.optional(&v.reference, Writer::reference)?;
            write_rpc(w, &v.rpc)?;
            v.fine_position.write(w, 10)?;
            v.fine_rotation.write(w)?;
            w.bit(v.fine_flag)?;
        }
        (Kind::Chassis, Initial::Chassis(v)) => {
            write_rpc(w, &v.rpc)?;
            for vec in &v.vectors {
                sparse_float_write(w, vec)?;
            }
            custom_write(w, &v.custom)?;
        }
        (Kind::Part { .. }, Initial::Part(value)) => w.bit(*value)?,
        (Kind::Bool | Kind::Mesh(_) | Kind::Empty, Initial::Empty) => (),
        (Kind::Wheel, Initial::Wheel { rpc, flag }) => {
            write_rpc(w, rpc)?;
            w.bit(*flag)?;
        }
        (Kind::SearchLight, Initial::SearchLight { rpc, enabled }) => {
            write_rpc(w, rpc)?;
            w.bit(*enabled)?;
        }
        (Kind::Siren, Initial::Siren { rpc, flags }) => {
            write_rpc(w, rpc)?;
            for flag in flags {
                w.bit(*flag)?;
            }
        }
        (Kind::Index, Initial::Index { rpc, index, value }) => {
            write_rpc(w, rpc)?;
            w.optional(index, |w, v| w.put(u64::from(*v), 4))?;
            w.put(u64::from(*value), 4)?;
        }
        (Kind::FourBit, Initial::FourBit(value)) => w.put(u64::from(*value), 4)?,
        (
            Kind::TripleNibbles
            | Kind::Tuning
            | Kind::Tagged
            | Kind::NibblesGuid
            | Kind::Appearance
            | Kind::GuidFloat
            | Kind::Rpc,
            Initial::Rpc(rpc),
        ) => write_rpc(w, rpc)?,
        _ => return Err(Error::TypeMismatch),
    }
    Ok(())
}
pub(super) fn update_read(r: &mut Reader<'_>, k: &Kind) -> Result<Update, Error> {
    Ok(match k {
        Kind::Root { .. } => Update::Root(r.optional(|r| Ok(r.take(5)? as u8))?),
        Kind::Chassis => Update::Chassis(Box::new(ChassisUpdate {
            physics: r.optional(physics_read)?,
            flag: r.optional(Reader::bit)?,
            custom: r.optional(custom_read)?,
            pair: r.optional(|r| Ok((r.take(32)?, r.bit()?)))?,
        })),
        Kind::Part { variants } => {
            Update::Part(r.optional(|r| Ok(r.take((16 - variants.leading_zeros()) as u8)? as u16))?)
        }
        Kind::TripleNibbles => Update::TripleNibbles(r.optional(|r| bytes(r, 4))?),
        Kind::Bool => Update::Bool(r.optional(Reader::bit)?),
        Kind::Wheel => Update::Wheel(r.optional(Reader::bit)?),
        Kind::SearchLight => Update::SearchLight(r.optional(Reader::bit)?),
        Kind::Siren => Update::Siren {
            flags: array(|| r.optional(Reader::bit))?,
            trigger: r.bit()?,
        },
        Kind::Index => Update::Index {
            index: r.optional(|r| Ok(r.take(2)? as u8))?,
            value: r.optional(|r| Ok(r.take(8)? as u8))?,
        },
        Kind::Tuning => Update::Tuning(r.optional(|r| {
            Ok(Tuning {
                values: words(r, 10)?,
                flags: array(|| r.bit())?,
                tail: words(r, 10)?,
            })
        })?),
        Kind::Tagged => Update::Tagged(r.optional(|r| {
            let tag = r.take(3)? as u8;
            Ok(Tagged {
                tag,
                reference: if tag == 3 { Some(r.reference()?) } else { None },
                pair: if (3..=5).contains(&tag) {
                    Some(array(|| r.take(32))?)
                } else {
                    None
                },
            })
        })?),
        Kind::NibblesGuid => Update::NibblesGuid(r.optional(|r| {
            Ok(NibblesGuid {
                head: bytes(r, 4)?,
                guid: array(|| r.take(32))?,
                tail: bytes(r, 4)?,
            })
        })?),
        Kind::FourBit | Kind::Rpc | Kind::Empty => Update::Noop,
        Kind::Appearance => Update::Appearance(Box::new(appearance_read(r)?)),
        Kind::Mesh(profile) => Update::Mesh(r.optional(|r| mesh_read(r, profile))?),
        Kind::GuidFloat => Update::GuidFloat {
            guid: r.optional(|r| array(|| r.take(32)))?,
            value: r.optional(|r| r.take(32))?,
        },
    })
}
pub(super) fn update_write(w: &mut Writer, k: &Kind, v: &Update) -> Result<(), Error> {
    match (k, v) {
        (Kind::Root { .. }, Update::Root(v)) => w.optional(v, |w, v| w.put(u64::from(*v), 5))?,
        (Kind::Chassis, Update::Chassis(c)) => {
            w.optional(&c.physics, physics_write)?;
            w.optional(&c.flag, |w, v| w.bit(*v))?;
            w.optional(&c.custom, custom_write)?;
            w.optional(&c.pair, |w, (value, flag)| {
                w.put(u64::from(*value), 32)?;
                w.bit(*flag)
            })?;
        }
        (Kind::Part { variants }, Update::Part(v)) => w.optional(v, |w, v| {
            w.put(u64::from(*v), (16 - variants.leading_zeros()) as usize)
        })?,
        (Kind::TripleNibbles, Update::TripleNibbles(v)) => {
            w.optional(v, |w, v| put_bytes(w, v, 4))?
        }
        (Kind::Bool, Update::Bool(v))
        | (Kind::Wheel, Update::Wheel(v))
        | (Kind::SearchLight, Update::SearchLight(v)) => w.optional(v, |w, v| w.bit(*v))?,
        (Kind::Siren, Update::Siren { flags, trigger }) => {
            for flag in flags {
                w.optional(flag, |w, v| w.bit(*v))?;
            }
            w.bit(*trigger)?;
        }
        (Kind::Index, Update::Index { index, value }) => {
            w.optional(index, |w, v| w.put(u64::from(*v), 2))?;
            w.optional(value, |w, v| w.put(u64::from(*v), 8))?;
        }
        (Kind::Tuning, Update::Tuning(v)) => w.optional(v, |w, t| {
            put_words(w, &t.values, 10)?;
            for f in t.flags {
                w.bit(f)?;
            }
            put_words(w, &t.tail, 10)
        })?,
        (Kind::Tagged, Update::Tagged(v)) => w.optional(v, |w, t| {
            if (t.tag == 3) != t.reference.is_some() || (3..=5).contains(&t.tag) != t.pair.is_some()
            {
                return Err(Error::Shape);
            }
            w.put(u64::from(t.tag), 3)?;
            if let Some(id) = t.reference {
                w.reference(&id)?;
            }
            if let Some(pair) = t.pair {
                put_dwords(w, &pair)?;
            }
            Ok(())
        })?,
        (Kind::NibblesGuid, Update::NibblesGuid(v)) => w.optional(v, |w, n| {
            put_bytes(w, &n.head, 4)?;
            put_dwords(w, &n.guid)?;
            put_bytes(w, &n.tail, 4)
        })?,
        (Kind::FourBit | Kind::Rpc | Kind::Empty, Update::Noop) => (),
        (Kind::Appearance, Update::Appearance(a)) => appearance_write(w, a)?,
        (Kind::Mesh(profile), Update::Mesh(v)) => {
            w.optional(v, |w, m| mesh_write(w, profile, m))?
        }
        (Kind::GuidFloat, Update::GuidFloat { guid, value }) => {
            w.optional(guid, |w, v| put_dwords(w, v))?;
            w.optional(value, |w, v| w.put(u64::from(*v), 32))?;
        }
        _ => return Err(Error::TypeMismatch),
    }
    Ok(())
}

#[cfg(test)]
mod codec_tests {
    use super::*;
    #[test]
    fn empty_serializer_keeps_its_presence_bit_and_has_no_payload() {
        let profile = Profile::new(vec![
            Kind::Root {
                property_owner: false,
            },
            Kind::Empty,
            Kind::Bool,
        ])
        .unwrap();
        let body = Body {
            creation: None,
            updates: vec![
                Some(Update::Root(None)),
                Some(Update::Noop),
                Some(Update::Bool(Some(true))),
            ],
        };
        // Root present, no root value; empty present; bool present, value present, true.
        let bytes = [0xbc]; // 10 1 111
        let span = BitSpan::new(&bytes, 0, 6).unwrap();
        assert_eq!(body.encode(&profile).unwrap().bytes(), bytes);
        let decoded = Body::decode(span, &profile, false).unwrap();
        assert_eq!(decoded.body, body);
        assert_eq!(decoded.bits, 6);
        for bits in 0..6 {
            assert_eq!(
                Body::decode(BitSpan::new(&bytes, 0, bits).unwrap(), &profile, false),
                Err(Error::Truncated)
            );
        }
        let mut writer = Writer(BitWriter::new());
        initial_write(&mut writer, &Kind::Empty, &Initial::Empty).unwrap();
        update_write(&mut writer, &Kind::Empty, &Update::Noop).unwrap();
        assert_eq!(writer.0.len(), 0);
        assert_eq!(
            initial_write(&mut writer, &Kind::Empty, &Initial::FourBit(0)),
            Err(Error::TypeMismatch)
        );
    }
    #[test]
    fn spoiler_packed_value_preserves_alignment_and_rejects_truncation() {
        // Five unrelated bits, signed 3-bit index -1, normalized u16 0x1234,
        // selector 0x56, signed 2-bit index 1, one asset reference 0x2345.
        let bytes = [0xaf, 0x12, 0x34, 0x56, 0x40, 0, 0x48, 0xd1, 0x40];
        let profile = [
            MeshKind {
                index_bits: 3,
                extra: MeshExtra::Spoiler,
            },
            MeshKind {
                index_bits: 2,
                extra: MeshExtra::None,
            },
        ];
        let span = BitSpan::new(&bytes, 5, 61).unwrap();
        let mut reader = Reader { span, pos: 0 };
        let value = mesh_read(&mut reader, &profile).unwrap();
        assert_eq!(reader.pos, 61);
        assert_eq!(
            value,
            MeshCustomization {
                meshes: vec![
                    Mesh {
                        index: -1,
                        extra: MeshValue::Spoiler {
                            normalized: 0x1234,
                            selector: 0x56
                        }
                    },
                    Mesh {
                        index: 1,
                        extra: MeshValue::None
                    }
                ],
                assets: vec![0x2345]
            }
        );
        let mut writer = Writer(BitWriter::new());
        writer.put(0b10101, 5).unwrap();
        mesh_write(&mut writer, &profile, &value).unwrap();
        assert_eq!(writer.0.len(), 66);
        assert_eq!(writer.0.bytes(), bytes);
        for bits in 0..61 {
            let mut reader = Reader {
                span: BitSpan::new(&bytes, 5, bits).unwrap(),
                pos: 0,
            };
            assert_eq!(mesh_read(&mut reader, &profile), Err(Error::Truncated));
        }
        let mut wrong = value;
        wrong.meshes[0].extra = MeshValue::Light(0);
        assert_eq!(
            mesh_write(&mut Writer(BitWriter::new()), &profile, &wrong),
            Err(Error::TypeMismatch)
        );
    }
}

#[cfg(test)]
mod rpc_tests {
    use super::*;

    #[test]
    fn rpc_only_component_consumes_exactly_nineteen_initial_bits_and_no_update_bits() {
        // selector 0x155 (9 bits), serial 0x2aa (10 bits), followed by marker 101.
        let bytes = [0xaa, 0xd5, 0x54];
        let expected = Initial::Rpc(Rpc {
            selector: 0x155,
            serial: Serial::new(0x2aa).unwrap(),
        });
        let mut r = Reader {
            span: BitSpan::new(&bytes, 0, 22).unwrap(),
            pos: 0,
        };
        assert_eq!(initial_read(&mut r, &Kind::Rpc).unwrap(), expected);
        assert_eq!(r.pos, 19);
        assert_eq!(update_read(&mut r, &Kind::Rpc).unwrap(), Update::Noop);
        assert_eq!(r.pos, 19);
        assert_eq!(r.take(3).unwrap(), 5);
        for bits in 0..19 {
            let mut r = Reader {
                span: BitSpan::new(&bytes, 0, bits).unwrap(),
                pos: 0,
            };
            assert_eq!(initial_read(&mut r, &Kind::Rpc), Err(Error::Truncated));
        }
        let mut w = Writer(BitWriter::new());
        initial_write(&mut w, &Kind::Rpc, &expected).unwrap();
        update_write(&mut w, &Kind::Rpc, &Update::Noop).unwrap();
        assert_eq!(w.0.len(), 19);
        w.put(5, 3).unwrap();
        assert_eq!(w.0.bytes(), bytes);
        assert_eq!(
            update_write(&mut w, &Kind::Rpc, &Update::Bool(None)),
            Err(Error::TypeMismatch)
        );
    }

    #[test]
    fn empty_component_updates_still_have_outer_presence_flags() {
        let p = Profile::new(vec![
            Kind::Root {
                property_owner: false,
            },
            Kind::Rpc,
            Kind::Bool,
        ])
        .unwrap();
        let body = Body {
            creation: None,
            updates: vec![None, Some(Update::Noop), Some(Update::Bool(Some(true)))],
        };
        let wire = body.encode(&p).unwrap();
        assert_eq!(wire.len(), 5);
        assert_eq!(wire.bytes(), [0x78]); // absent root, present RPC, present bool, set/value.
        assert_eq!(Body::decode(wire.span(), &p, false).unwrap().body, body);
        assert_eq!(
            Body::decode(BitSpan::new(wire.bytes(), 0, 1).unwrap(), &p, false),
            Err(Error::Truncated)
        );
    }
}
