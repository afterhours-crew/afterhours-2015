// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

#[test]
fn owned_power_figures_are_separate_and_update_atomically_per_session() {
    let p = Arc::new(Program {
        nodes: vec![node(0, Behavior::Stats)],
        cells: [stats::MAX_HP, stats::STOCK_MAX_HP, stats::TOP_SPEED]
            .into_iter()
            .map(|field| cell(Kind::Integer, &[port(0, field)]))
            .collect(),
        connections: vec![],
        startup: vec![Step::Bind(0), Step::Activate(0)],
        unmodeled: vec![],
    });
    let mut first = Session::new(p.clone()).unwrap();
    let mut second = Session::new(p).unwrap();
    let mut sample = inputs();
    first.start(&sample).unwrap();
    sample.participant += 1;
    second.start(&sample).unwrap();
    assert_eq!(first.read(port(0, stats::MAX_HP)).unwrap(), None);
    sample.participant -= 1;
    sample.stats = Some(stats::Stats {
        max_hp: 355,
        stock_max_hp: 205,
        top_speed_mph: 175,
        stock_top_speed_mph: 155,
        zero_to_sixty: 4.1,
        stock_zero_to_sixty: 6.8,
    });
    first.tick(0.1, &sample).unwrap();
    assert_eq!(
        first.read(port(0, stats::MAX_HP)).unwrap(),
        Some(Value::Integer(355))
    );
    assert_eq!(
        first.read(port(0, stats::STOCK_MAX_HP)).unwrap(),
        Some(Value::Integer(205))
    );
    assert_eq!(second.read(port(0, stats::MAX_HP)).unwrap(), None);
    assert_eq!(second.read(port(0, stats::STOCK_MAX_HP)).unwrap(), None);
    // A car change updates the baseline as well as the tuned value.
    sample.stats.as_mut().unwrap().max_hp = 270;
    sample.stats.as_mut().unwrap().stock_max_hp = 220;
    first.tick(0.1, &sample).unwrap();
    assert_eq!(
        first.read(port(0, stats::MAX_HP)).unwrap(),
        Some(Value::Integer(270))
    );
    assert_eq!(
        first.read(port(0, stats::STOCK_MAX_HP)).unwrap(),
        Some(Value::Integer(220))
    );
    sample.stats.as_mut().unwrap().max_hp = 1501;
    sample.stats.as_mut().unwrap().top_speed_mph = 190;
    assert_eq!(first.tick(0.1, &sample), Err(Error::Bound));
    assert_eq!(
        first.read(port(0, stats::MAX_HP)).unwrap(),
        Some(Value::Integer(270))
    );
    assert_eq!(
        first.read(port(0, stats::TOP_SPEED)).unwrap(),
        Some(Value::Integer(175))
    );
    sample.stats.as_mut().unwrap().max_hp = 310;
    first.tick(0.1, &sample).unwrap();
    assert_eq!(
        first.read(port(0, stats::MAX_HP)).unwrap(),
        Some(Value::Integer(310))
    );
}
