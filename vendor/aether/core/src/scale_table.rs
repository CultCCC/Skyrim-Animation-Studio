//! The `BSPSysScaleModifier` size table, in ONE place.
//!
//! The format stores a size curve as N values spread evenly over the
//! particle's normalized life, and the game reads it with
//! `BSTableCurve::EvaluateNormalized` (1.5.97 `0x140D689E0`): clamp to
//! the ends, then lerp between the two nearest samples. That is exactly
//! [`sample`], and every other operation here is phrased in terms of it.
//!
//! Before this module the same comma list was parsed in FOUR places
//! (the compiler, the simulator, the node body and a dead strip), each
//! with its own idea of an empty list.
//!
//! The tables vanilla ships are DENSE -- measured over the corpus, 3,185
//! nodes, most between 40 and 200 samples and some past 280. They are
//! baked curves, not hand-placed values, which decides the editing
//! model: painting one sample per frame cannot keep up with a mouse
//! across that many columns ([`paint`] fills the whole stroke), and
//! adding a sample at the end retimes every one before it ([`resample`]
//! keeps the shape instead). The Blender-style control-point view
//! ([`fit`] / [`bake`]) exists for the same reason: a dense table is
//! easier to reshape through a handful of points than through 200
//! columns.

/// Parse the stored comma list. Unparsable entries are dropped, the way
/// every reader has always treated them; an empty result is the
/// CALLER's to interpret, because they disagree on purpose (the
/// compiler writes a flat 1.0 table, the simulator applies no scale).
pub fn parse(text: &str) -> Vec<f32> {
    text.split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

/// The stored form. Exactly what import writes (`{v}` joined by ", "),
/// so parsing and formatting a file's own table is the identity -- a
/// property pinned against every fixture, because the edit sweep and the
/// null-edit gate both rest on it.
pub fn format(values: &[f32]) -> String {
    values
        .iter()
        .map(|v| format!("{v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The game's own sampler: `BSTableCurve::EvaluateNormalized`.
pub fn sample(values: &[f32], t: f32) -> f32 {
    match values.len() {
        0 => 1.0,
        1 => values[0],
        n => {
            if t >= 1.0 {
                return values[n - 1];
            }
            let t = t.max(0.0);
            let x = (n - 1) as f32 * t;
            let i = x.floor();
            let idx = i as usize;
            values[idx] + (values[idx + 1] - values[idx]) * (x - i)
        }
    }
}

/// The normalized life a sample index sits at.
pub fn sample_time(i: usize, n: usize) -> f32 {
    if n < 2 {
        0.0
    } else {
        i as f32 / (n - 1) as f32
    }
}

/// The same curve at a different sample count.
///
/// NOT a push or a pop: samples are spread over the WHOLE life, so
/// appending one moves every other sample earlier and the curve shrinks
/// in time -- which is what the node's old `+` did. Re-sampling through
/// [`sample`] keeps the shape. At the same count it returns the table
/// untouched, bit for bit, so asking for the count a table already has
/// can never rewrite it.
pub fn resample(values: &[f32], n: usize) -> Vec<f32> {
    let n = n.max(2);
    if n == values.len() {
        return values.to_vec();
    }
    (0..n).map(|i| sample(values, sample_time(i, n))).collect()
}

/// Paint a stroke from `(t0, v0)` to `(t1, v1)`: EVERY sample whose time
/// falls between the two is set on the line joining them.
///
/// A pointer reports one position per frame. At 70 samples across a
/// 230 px strip a column is ~3 px wide, and a brisk stroke crosses
/// several between two frames; setting only the sample under the
/// pointer left the skipped ones at their old values, which is the row
/// of spikes the user drew. The stroke is the segment between the last
/// two pointer positions, and it is filled whole.
///
/// Returns whether anything changed.
pub fn paint(values: &mut [f32], (t0, v0): (f32, f32), (t1, v1): (f32, f32)) -> bool {
    let n = values.len();
    if n == 0 {
        return false;
    }
    let (t0, t1) = (t0.clamp(0.0, 1.0), t1.clamp(0.0, 1.0));
    let index = |t: f32| ((t * (n - 1) as f32).round() as usize).min(n - 1);
    let (i0, i1) = (index(t0), index(t1));
    let mut changed = false;
    let mut set = |i: usize, v: f32| {
        if values[i] != v {
            values[i] = v;
            changed = true;
        }
    };
    if i0 == i1 || (t1 - t0).abs() < f32::EPSILON {
        set(i1, v1);
        return changed;
    }
    for i in i0.min(i1)..=i0.max(i1) {
        let f = ((sample_time(i, n) - t0) / (t1 - t0)).clamp(0.0, 1.0);
        set(i, v0 + (v1 - v0) * f);
    }
    changed
}

/// One pass of a 1-2-1 blur, ends held. Repeatable: each press takes
/// out a little more of the stair-stepping a hand-painted table has.
pub fn smooth(values: &mut [f32]) {
    if values.len() < 3 {
        return;
    }
    let src = values.to_vec();
    for i in 1..src.len() - 1 {
        values[i] = (src[i - 1] + 2.0 * src[i] + src[i + 1]) * 0.25;
    }
}

/// A control point of the Blender-style view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// Normalized life, 0 = birth, 1 = death.
    pub t: f32,
    /// Size multiplier.
    pub v: f32,
}

/// The few points that describe a dense table, within `tolerance`.
///
/// Ramer-Douglas-Peucker over the samples: keep the ends, then keep the
/// sample farthest from the line between the kept neighbours until
/// nothing strays more than `tolerance` in VALUE. Measured in value
/// rather than as a distance on screen, because the file does not know
/// how wide the strip is.
pub fn fit(values: &[f32], tolerance: f32) -> Vec<Point> {
    let n = values.len();
    if n == 0 {
        return vec![Point { t: 0.0, v: 1.0 }, Point { t: 1.0, v: 1.0 }];
    }
    if n == 1 {
        return vec![Point { t: 0.0, v: values[0] }, Point { t: 1.0, v: values[0] }];
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (va, vb) = (values[a], values[b]);
        let mut worst = (0usize, 0.0f32);
        for i in a + 1..b {
            let f = (i - a) as f32 / (b - a) as f32;
            let d = (values[i] - (va + (vb - va) * f)).abs();
            if d > worst.1 {
                worst = (i, d);
            }
        }
        if worst.1 > tolerance {
            keep[worst.0] = true;
            stack.push((a, worst.0));
            stack.push((worst.0, b));
        }
    }
    // RDP bounds the error of STRAIGHT segments, and the points are
    // joined by a cubic. Measured on ZD_MarkartWarhammer's 8-sample
    // table, the cubic through the RDP points strays 7.8% of the range
    // from the table it was fitted to. So the fit is finished against
    // the curve that will actually be drawn and baked: keep adding the
    // worst sample until the CUBIC is within tolerance everywhere. In
    // the limit every sample is a point, and a monotone cubic through
    // every sample reproduces the table exactly.
    loop {
        let points: Vec<Point> = (0..n)
            .filter(|i| keep[*i])
            .map(|i| Point { t: sample_time(i, n), v: values[i] })
            .collect();
        let worst = (0..n)
            .filter(|i| !keep[*i])
            .map(|i| (i, (eval(&points, sample_time(i, n)) - values[i]).abs()))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match worst {
            Some((i, d)) if d > tolerance => keep[i] = true,
            _ => return points,
        }
    }
}

/// Write a control-point edit back into the table, touching only the
/// samples where the curve actually CHANGED.
///
/// The points describe the table within a tolerance, not exactly. Baking
/// the whole table on every edit would replace every untouched sample
/// with its approximation, so moving one point near the end of life
/// would nudge the start of it too -- an edit nobody made. Comparing the
/// curve before and after the change instead of reasoning about which
/// segments a point can reach is exact by construction, including where
/// the monotone tangents couple neighbouring segments.
pub fn rebake(values: &mut [f32], before: &[Point], after: &[Point]) {
    let n = values.len();
    for (i, v) in values.iter_mut().enumerate() {
        let t = sample_time(i, n);
        let new = eval(after, t);
        if new.to_bits() != eval(before, t).to_bits() {
            *v = new;
        }
    }
}

/// The value of a control-point curve at `t`: a monotone cubic
/// (Fritsch-Carlson) through the points.
///
/// Monotone because a size curve must not OVERSHOOT: an ordinary spline
/// through a sharp rise dips below its own lower point before it, and a
/// size that dips below zero is a particle turned inside out. Blender's
/// "auto clamped" handles make the same promise for the same reason.
/// Flat outside the first and last point.
pub fn eval(points: &[Point], t: f32) -> f32 {
    match points.len() {
        0 => return 1.0,
        1 => return points[0].v,
        _ => {}
    }
    if t <= points[0].t {
        return points[0].v;
    }
    let last = points.len() - 1;
    if t >= points[last].t {
        return points[last].v;
    }
    let k = points.windows(2).position(|w| t < w[1].t).unwrap_or(last - 1);
    let slopes = tangents(points);
    let (p0, p1) = (points[k], points[k + 1]);
    let h = (p1.t - p0.t).max(f32::EPSILON);
    let s = (t - p0.t) / h;
    let (s2, s3) = (s * s, s * s * s);
    (2.0 * s3 - 3.0 * s2 + 1.0) * p0.v
        + (s3 - 2.0 * s2 + s) * h * slopes[k]
        + (-2.0 * s3 + 3.0 * s2) * p1.v
        + (s3 - s2) * h * slopes[k + 1]
}

/// Fritsch-Carlson tangents: secant averages, zeroed at a local extreme
/// and scaled back wherever they would overshoot.
fn tangents(points: &[Point]) -> Vec<f32> {
    let n = points.len();
    let secant: Vec<f32> = points
        .windows(2)
        .map(|w| (w[1].v - w[0].v) / (w[1].t - w[0].t).max(f32::EPSILON))
        .collect();
    let mut m = vec![0.0f32; n];
    m[0] = secant[0];
    m[n - 1] = secant[n - 2];
    for i in 1..n - 1 {
        m[i] = if secant[i - 1] * secant[i] <= 0.0 {
            0.0
        } else {
            (secant[i - 1] + secant[i]) * 0.5
        };
    }
    for i in 0..n - 1 {
        if secant[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let (a, b) = (m[i] / secant[i], m[i + 1] / secant[i]);
        let r = a * a + b * b;
        if r > 9.0 {
            let tau = 3.0 / r.sqrt();
            m[i] = tau * a * secant[i];
            m[i + 1] = tau * b * secant[i];
        }
    }
    m
}

/// The table a control-point curve becomes at `n` samples.
pub fn bake(points: &[Point], n: usize) -> Vec<f32> {
    let n = n.max(2);
    (0..n).map(|i| eval(points, sample_time(i, n))).collect()
}

/// Starting shapes, named by what they do to a particle rather than by
/// the curve's geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Size 1.0 all life.
    Constant,
    /// From nothing at birth to full size at death, eased.
    Grow,
    /// From full size at birth to nothing at death, eased.
    Shrink,
    /// Swells to full size at mid-life and back to nothing.
    Swell,
    /// Pops to full size in the first tenth of life, then holds.
    Pop,
}

pub const PRESETS: &[(Preset, &str)] = &[
    (Preset::Constant, "Constant"),
    (Preset::Grow, "Grow"),
    (Preset::Shrink, "Shrink"),
    (Preset::Swell, "Swell"),
    (Preset::Pop, "Pop"),
];

impl Preset {
    /// What the preset does to a particle, for its hover.
    pub fn describe(self) -> &'static str {
        match self {
            Preset::Constant => "Size ×1 for the whole life: the particle keeps its spawn size.",
            Preset::Grow => "Born at nothing, eases up to full size by death. Smoke that spreads as it rises.",
            Preset::Shrink => "Born at full size, eases down to nothing by death. Sparks and embers burning out.",
            Preset::Swell => "Grows from nothing to full size at mid-life, then shrinks away again. Puffs and pops.",
            Preset::Pop => "Reaches full size in the first tenth of life, then holds. Flashes and bursts.",
        }
    }

    pub fn points(self) -> Vec<Point> {
        let p = |t, v| Point { t, v };
        match self {
            Preset::Constant => vec![p(0.0, 1.0), p(1.0, 1.0)],
            Preset::Grow => vec![p(0.0, 0.0), p(0.5, 0.75), p(1.0, 1.0)],
            Preset::Shrink => vec![p(0.0, 1.0), p(0.5, 0.75), p(1.0, 0.0)],
            Preset::Swell => vec![p(0.0, 0.0), p(0.5, 1.0), p(1.0, 0.0)],
            Preset::Pop => vec![p(0.0, 0.0), p(0.1, 1.0), p(1.0, 1.0)],
        }
    }
}
