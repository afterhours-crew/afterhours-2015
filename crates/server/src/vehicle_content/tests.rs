// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_world::items::{DefinitionClass as Class, Derived, Item, OWNED};
use serde_json::json;

fn guid(id: u8) -> String {
    format!("{id:02x}").repeat(16)
}
pub(crate) fn fixture() -> (Value, Catalog, Collection) {
    let classes = [
        Class::RaceVehicleItemData,
        Class::RimsItemData,
        Class::RimsItemData,
        Class::TiresItemData,
        Class::TiresItemData,
    ];
    let catalog = Catalog::new(
        classes
            .into_iter()
            .enumerate()
            .map(|(i, c)| ([(i + 1) as u8; 16], c)),
    )
    .unwrap();
    let mut items = BTreeMap::new();
    for (i, class) in classes.into_iter().enumerate() {
        let id = i as u64 + 1;
        let n = match class {
            Class::RaceVehicleItemData => 68,
            Class::RimsItemData => 52,
            _ => 4,
        };
        items.insert(
            id,
            Item {
                id,
                definition: [id as u8; 16],
                owner: if id == 1 { 0 } else { 1 },
                defaults: vec![],
                children: vec![],
                state: OWNED,
                buy_price: 0,
                sell_price: 0,
                derived: Derived::decode(class.layout(), &vec![0; n]).unwrap(),
            },
        );
    }
    items.get_mut(&1).unwrap().children = vec![2, 3, 4, 5];
    let custom=(1..=5).map(|i|json!({"guid":guid(i),"class_name":classes[i as usize-1].name(),"tuning":{},"chassis":
        if i==1{json!({"family":"other"})}else if i<=3{json!({"family":"customization","flag":false,"rim_values":[0.5],"rear":i==3})}else{json!({"family":"customization","flag":false})}
    })).collect::<Vec<_>>();
    let wheels=(2..=5).map(|i|if i<=3{json!({"guid":guid(i),"class_name":"RimsItemData","rear":i==3,"diameter":0.5,"width":0.25,"scale":0.5,"sizes":[0.5],"flag":false})}
        else{json!({"guid":guid(i),"class_name":"TiresItemData","rear":i==5,"alternatives":[[0.5,0.7,0.25]]})}).collect::<Vec<_>>();
    let meshes=(1..=5).map(|i|if i<=1{json!({"guid":guid(i),"class_name":classes[i as usize-1].name(),"kind":"ignore"})}
        else{json!({"guid":guid(i),"class_name":classes[i as usize-1].name(),"kind":"part","guids":[guid(21)],"rim":null})}).collect::<Vec<_>>();
    let mut profile = vec![
        json!({"kind":"root","property_owner":true}),
        json!({"kind":"chassis"}),
        json!({"kind":"part","variants":1}),
    ];
    for kind in [
        "triple_nibbles",
        "bool",
        "wheel",
        "wheel",
        "wheel",
        "wheel",
        "index",
        "tuning",
        "tagged",
        "nibbles_guid",
        "four_bit",
        "appearance",
    ] {
        profile.push(json!({"kind":kind}));
    }
    profile.extend([
        json!({"kind":"mesh","entries":[{"index_bits":2,"extra":"none"}]}),
        json!({"kind":"guid_float"}),
    ]);
    let vehicle = json!({"definitions":[guid(1)],"blueprint":"car/test","bundle":"car/test_bundle","asset":{"type_id":3410,"local_index":0},"catalog":[[3410,1]],
        "profile":profile,"parts":[{"ordinal":2,"variants":1,"networkable":true}],"buses":[{"path":[],"flags":1}],
        "resting":{"mass":0.,"front_axle":1.,"wheelbase":2.,"curve":{"min":[0.,0.],"max":[7.,100.],"points":(0..8).map(|i|[i as f32/7.,0.5]).collect::<Vec<_>>()},"front":[0.,100.,0.1],"rear":[0.,100.,0.1]},
        "team":0,"mode":0,"wheel_baseline":[0.7,0.7],"appearance":{"palettes":([[[0.;3];4];2]),"static_wrap_counts":null},
        "meshes":[{"index_bits":2,"extra":"none","variants":[{"guid":guid(21),"bundle":"wheel/test"}]}],"health":{"maximum":100.,"profile":null}});
    (
        json!({"format":"nfs-vehicle-content","version":1,"build_sha256":BUILD,"vehicles":[vehicle],
        "parts":{"customization":custom,"nos":[],"wheels":wheels,"appearance":[
            {"guid":guid(2),"class_name":"RimsItemData","rear":false},{"guid":guid(3),"class_name":"RimsItemData","rear":true}],"mesh":meshes}}),
        catalog,
        Collection {
            roots: vec![1],
            items,
        },
    )
}
fn request(items: &Collection) -> Request<'_> {
    Request {
        items,
        vehicle: 1,
        locator: [1., 2., 3.],
        basis: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        origin: [0.; 3],
        speed: None,
        connection: Some(3),
        owner: Resource {
            words: [10, 0],
            name: b"Local".to_vec(),
        },
        customization_attached: true,
        world: false,
        authority_group: 10,
        authority_components: &[100],
    }
}

#[test]
fn fresh_parked_construction_uses_current_owner_and_keeps_independent_worlds() {
    let (json, catalog, items) = fixture();
    let content = Content::from_json(&json, &catalog).unwrap();
    let mut a = authority::Registry::default();
    let body = content
        .construct(request(&items), &mut a, |_| Some(7))
        .unwrap();
    let before = a.clone();
    assert_eq!(
        content.construct(request(&items), &mut a, |_| Some(7)),
        Ok(body.clone())
    );
    assert_eq!(a, before);
    let c = body.creation.as_ref().unwrap();
    assert_eq!(c.connection_id, 3);
    assert!(matches!(&c.fields[0],Initial::Root(r) if r.fine_flag));
    assert_eq!(body.updates.len(), 17);
    let mut b = authority::Registry::default();
    let mut other = request(&items);
    other.connection = Some(9);
    other.customization_attached = false;
    other.origin = [1., 0., 0.];
    other.owner.words = [20, 0];
    let separate = content.construct(other, &mut b, |_| Some(11)).unwrap();
    assert_eq!(separate.creation.as_ref().unwrap().connection_id, 9);
    assert_ne!(separate, body);
    assert_eq!(
        content.construct(request(&items), &mut a, |_| Some(7)),
        Ok(body)
    );
}

#[test]
fn late_missing_asset_or_bad_items_roll_back_authority_registration() {
    let (json, catalog, mut items) = fixture();
    let content = Content::from_json(&json, &catalog).unwrap();
    let mut authority = authority::Registry::default();
    assert_eq!(
        content.construct(request(&items), &mut authority, |_| None),
        Err(Error::UnknownObject)
    );
    assert_eq!(authority, authority::Registry::default());
    items.items.get_mut(&2).unwrap().owner = 99;
    assert!(
        content
            .construct(request(&items), &mut authority, |_| Some(7))
            .is_err()
    );
    assert_eq!(authority, authority::Registry::default());
}

#[test]
fn loader_rejects_dynamic_fields_wrong_catalogs_and_inconsistent_static_shapes() {
    let (original, catalog, _) = fixture();
    Content::from_json(&original, &catalog).unwrap();
    let mut cases = Vec::new();
    let mut v = original.clone();
    v["owner"] = json!(5);
    cases.push(v);
    let mut v = original.clone();
    v["version"] = json!(2);
    cases.push(v);
    let mut v = original.clone();
    v["vehicles"][0]["definitions"] = json!([guid(1), guid(1)]);
    cases.push(v);
    let mut v = original.clone();
    v["vehicles"][0]["parts"][0]["variants"] = json!(2);
    cases.push(v);
    let mut v = original.clone();
    v["vehicles"][0]["meshes"][0]["extra"] = json!("rim");
    cases.push(v);
    let mut v = original.clone();
    v["vehicles"][0]["resting"]["mass"] = json!(1e300);
    cases.push(v);
    let mut v = original.clone();
    v["vehicles"][0]["buses"][0]["path"] = json!(vec![0; 32]);
    cases.push(v);
    let mut v = original.clone();
    v["parts"]["wheels"][0]["class_name"] = json!("TiresItemData");
    cases.push(v);
    let mut v = original.clone();
    v["parts"]["nos"] = json!([{"guid":guid(1),"variant":0}]);
    cases.push(v);
    let mut v = original;
    v["vehicles"][0]["health"]["captured_health"] = json!(1);
    cases.push(v);
    for (index, v) in cases.into_iter().enumerate() {
        assert!(Content::from_json(&v, &catalog).is_err(), "case {index}");
    }
}
