//! A level's rails, for grinding: straight segments between rail nodes,
//! joined where they share an end.

use std::collections::HashSet;

use glam::Vec3;

/// One straight piece of rail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub start: Vec3,
    pub end: Vec3,
}

impl Segment {
    pub fn direction(&self) -> Vec3 {
        (self.end - self.start).normalize_or_zero()
    }

    pub fn length(&self) -> f32 {
        self.start.distance(self.end)
    }
}

/// Where the skater could get onto a rail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RailHit {
    pub segment: usize,
    /// The point on the rail.
    pub point: Vec3,
}

#[derive(Clone, Debug, Default)]
pub struct Rails {
    pub segments: Vec<Segment>,
    /// Each segment's node, the one whose link made it (its name, 0 for
    /// none): grinding onto the segment reaches it, running its
    /// `TriggerScript` (a rail gap's start or end, a goal's grind).
    pub nodes: Vec<u32>,
    /// How many segments, at the end, move (rails on moving objects: the
    /// toy cars, the Treehouse's lifeboat, the Hub's crane), put where they
    /// are each frame by whoever moves them.
    pub moving: usize,
    /// Rail nodes flagged `LipOverride`, by name: lips from them may be
    /// twisted up to `LipAllowAngle_Override` (main.dol 0x800E4568 sets
    /// the node's bit, 0x80108660 reads it).
    pub lip_override: HashSet<u32>,
    /// Rail nodes not there now, by name: not `CreatedAtStart`, or killed
    /// by a script, and not since created (bit 4 of the node's flags,
    /// main.dol 0x801CFC74). Their segments can't be got onto or
    /// grinded into.
    pub absent: HashSet<u32>,
}

/// Ends closer than this are the same node.
const JOIN: f32 = 0.01;

impl Rails {
    pub fn new(segments: Vec<Segment>) -> Self {
        Self::named(segments.into_iter().map(|s| (s, 0)).collect())
    }

    /// Rails from segments and their nodes' names (see [`Rails::nodes`]).
    pub fn named(segments: Vec<(Segment, u32)>) -> Self {
        // Nodes linked both ways round make the same segment twice: once
        // is enough (twice, the second is a join back the way it came),
        // keeping a node's name if either has one.
        let mut kept: Vec<Segment> = Vec::with_capacity(segments.len());
        let mut nodes: Vec<u32> = Vec::with_capacity(segments.len());
        for (s, node) in segments {
            let same = |k: &Segment| {
                (k.start.distance(s.start) < JOIN && k.end.distance(s.end) < JOIN)
                    || (k.start.distance(s.end) < JOIN && k.end.distance(s.start) < JOIN)
            };
            if s.length() <= JOIN {
                continue;
            }
            match kept.iter().position(same) {
                Some(i) => {
                    if nodes[i] == 0 {
                        nodes[i] = node;
                    }
                }
                None => {
                    kept.push(s);
                    nodes.push(node);
                }
            }
        }
        Rails {
            segments: kept,
            nodes,
            moving: 0,
            lip_override: HashSet::new(),
            absent: HashSet::new(),
        }
    }

    /// Segment `i`'s node's name, if it has one.
    /// Adds rails that move (after the level's own), each with its node;
    /// their segments are then set each frame with [`Rails::segments`].
    pub fn add_moving(&mut self, segments: Vec<(Segment, u32)>) {
        for (segment, node) in segments {
            self.segments.push(segment);
            self.nodes.push(node);
            self.moving += 1;
        }
    }

    /// Flags the named rail nodes `LipOverride`.
    pub fn with_lip_override(mut self, nodes: impl IntoIterator<Item = u32>) -> Self {
        self.lip_override.extend(nodes.into_iter().filter(|&n| n != 0));
        self
    }

    /// Whether segment `i`'s node is flagged `LipOverride`.
    pub fn lip_override(&self, i: usize) -> bool {
        self.node(i).is_some_and(|n| self.lip_override.contains(&n))
    }

    /// Creates (or kills) the named rail node, as a script's `Create`.
    pub fn set_present(&mut self, node: u32, present: bool) {
        if present {
            self.absent.remove(&node);
        } else if node != 0 {
            self.absent.insert(node);
        }
    }

    /// Whether segment `i`'s node is there.
    pub fn present(&self, i: usize) -> bool {
        self.node(i).is_none_or(|n| !self.absent.contains(&n))
    }

    /// Whether segment `i` moves.
    pub fn is_moving(&self, i: usize) -> bool {
        i < self.segments.len() && i + self.moving >= self.segments.len()
    }

    pub fn node(&self, i: usize) -> Option<u32> {
        self.nodes.get(i).copied().filter(|&n| n != 0)
    }

    /// The rail to grind on moving from `from` to `to` (main.dol
    /// 0x800E52C4): for each segment that's there, the closest points between it and
    /// the move; the best scores `distance * (1.122 - |cos|)`, with `cos`
    /// between the move and the rail, and it's taken if
    /// `distance * (2 - |cos|)` is within `max_snap`.
    pub fn nearest(&self, from: Vec3, to: Vec3, max_snap: f32) -> Option<RailHit> {
        let moved = (to - from).normalize_or_zero();
        let reach = Vec3::splat(max_snap);
        let (lo, hi) = (from.min(to) - reach, from.max(to) + reach);
        let mut best_score = f32::MAX;
        let mut best = None;
        for (i, segment) in self.segments.iter().enumerate() {
            if !self.present(i) {
                continue;
            }
            let (s_lo, s_hi) = (
                segment.start.min(segment.end),
                segment.start.max(segment.end),
            );
            if s_lo.cmpgt(hi).any() || s_hi.cmplt(lo).any() {
                continue;
            }
            let (on_move, on_rail) = closest_points(from, to, segment.start, segment.end);
            let distance = on_move.distance(on_rail);
            let mut cos = moved.dot(segment.direction()).abs();
            if moved == Vec3::ZERO {
                cos = 1.0;
            }
            let score = distance * (1.122 - cos);
            if score >= best_score {
                continue;
            }
            best_score = score;
            // The best so far, but out of reach: nothing, unless a later
            // one scores better.
            best = (distance * (2.0 - cos) <= max_snap).then_some(RailHit {
                segment: i,
                point: on_rail,
            });
        }
        best
    }

    /// The segments joined to `segment`'s end (or its start, going
    /// backwards), and whether each runs the same way (its start at that
    /// end).
    pub fn joins(
        &self,
        segment: usize,
        forwards: bool,
    ) -> impl Iterator<Item = (usize, bool)> + '_ {
        let this = self.segments[segment];
        let at = if forwards { this.end } else { this.start };
        let back = if forwards { this.start } else { this.end };
        self.segments
            .iter()
            .enumerate()
            .filter(move |&(i, _)| i != segment)
            .filter_map(move |(i, s)| {
                let same = if s.start.distance(at) < JOIN {
                    true
                } else if s.end.distance(at) < JOIN {
                    false
                } else {
                    return None;
                };
                // Not the one just left, going back.
                let other = if same { s.end } else { s.start };
                (other.distance(back) > JOIN).then_some((i, same))
            })
    }

    /// Of the segments there joined on at `segment`'s end (or start,
    /// going backwards), the one carrying on straightest; and whether it
    /// runs the same way.
    pub fn next(&self, segment: usize, forwards: bool) -> Option<(usize, bool)> {
        let travel = self.segments[segment].direction() * if forwards { 1.0 } else { -1.0 };
        self.joins(segment, forwards).filter(|&(i, _)| self.present(i)).max_by(|&(a, sa), &(b, sb)| {
            let along = |i: usize, same: bool| {
                self.segments[i].direction().dot(travel) * if same { 1.0 } else { -1.0 }
            };
            along(a, sa).total_cmp(&along(b, sb))
        })
    }
}

/// The closest points between segments `a0`-`a1` and `b0`-`b1`.
fn closest_points(a0: Vec3, a1: Vec3, b0: Vec3, b1: Vec3) -> (Vec3, Vec3) {
    let d1 = a1 - a0;
    let d2 = b1 - b0;
    let r = a0 - b0;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    let (s, t);
    if a <= f32::EPSILON && e <= f32::EPSILON {
        return (a0, b0);
    }
    if a <= f32::EPSILON {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= f32::EPSILON {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s0 = if denom != 0.0 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = s0;
            t = t0;
        }
    }
    (a0 + d1 * s, b0 + d2 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rails() -> Rails {
        // An L: along +X, then a right angle along +Z, then a gentle bend.
        Rails::new(vec![
            Segment {
                start: Vec3::new(0.0, 20.0, 0.0),
                end: Vec3::new(100.0, 20.0, 0.0),
            },
            Segment {
                start: Vec3::new(100.0, 20.0, 0.0),
                end: Vec3::new(100.0, 20.0, 100.0),
            },
        ])
    }

    #[test]
    fn finds_the_rail_the_skater_drops_onto() {
        let r = rails();
        // Falling along the rail, just above it.
        let hit = r
            .nearest(Vec3::new(40.0, 30.0, 2.0), Vec3::new(50.0, 25.0, 2.0), 40.0)
            .unwrap();
        assert_eq!(hit.segment, 0);
        assert!((hit.point - Vec3::new(50.0, 20.0, 0.0)).length() < 1e-3);
        // Too far away.
        assert!(
            r.nearest(Vec3::new(40.0, 90.0, 2.0), Vec3::new(50.0, 85.0, 2.0), 40.0)
                .is_none()
        );
    }

    #[test]
    fn joins_segments_at_shared_ends() {
        let r = rails();
        assert_eq!(r.next(0, true), Some((1, true)));
        assert_eq!(r.next(1, false), Some((0, false)));
        assert_eq!(r.next(0, false), None);
    }

    #[test]
    fn rails_not_there_are_neither_found_nor_followed() {
        let segments = rails().segments;
        let mut r = Rails::named(vec![(segments[0], 1), (segments[1], 2)]);
        let (from, to) = (Vec3::new(40.0, 30.0, 2.0), Vec3::new(50.0, 25.0, 2.0));
        r.set_present(1, false);
        r.set_present(2, false);
        assert!(r.nearest(from, to, 40.0).is_none());
        r.set_present(1, true);
        assert_eq!(r.nearest(from, to, 40.0).map(|h| h.segment), Some(0));
        assert_eq!(r.next(0, true), None);
        r.set_present(2, true);
        assert_eq!(r.next(0, true), Some((1, true)));
    }

    #[test]
    fn rails_linked_both_ways_are_one_and_branches_go_straightest() {
        let p = |x: f32, z: f32| Vec3::new(x, 20.0, z);
        let r = Rails::new(vec![
            Segment {
                start: p(0.0, 0.0),
                end: p(100.0, 0.0),
            },
            // The same, linked back.
            Segment {
                start: p(100.0, 0.0),
                end: p(0.0, 0.0),
            },
            // A branch off to the side, then the rail carrying on.
            Segment {
                start: p(100.0, 0.0),
                end: p(100.0, 100.0),
            },
            Segment {
                start: p(200.0, 10.0),
                end: p(100.0, 0.0),
            },
        ]);
        assert_eq!(r.segments.len(), 3);
        assert_eq!(r.next(0, true), Some((2, false)));
        assert_eq!(r.joins(0, true).count(), 2);
        // A name from whichever way round had one.
        let named = Rails::named(vec![
            (
                Segment {
                    start: p(0.0, 0.0),
                    end: p(100.0, 0.0),
                },
                0,
            ),
            (
                Segment {
                    start: p(100.0, 0.0),
                    end: p(0.0, 0.0),
                },
                7,
            ),
        ]);
        assert_eq!(named.segments.len(), 1);
        assert_eq!(named.node(0), Some(7));
        assert_eq!(Rails::new(named.segments.clone()).node(0), None);
    }
}
