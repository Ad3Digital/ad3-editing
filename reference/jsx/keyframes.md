# Keyframes

A `<keyframeTrack>` animates one prop of the element holding it over the node's local time; its `<keyframe>` children are the values along the way:

```tsx
<image src="stills/photo.jpg" start={0} end={5}>
  <keyframeTrack property="x">
    <keyframe time={0} value={-400} easing="easeOut" />
    <keyframe time={1} value={200} />
  </keyframeTrack>
  <keyframeTrack property="opacity">
    <keyframe time={0} value={0} />
    <keyframe time="15f" value={1} />
  </keyframeTrack>
</image>
```

| Element | Props | Meaning |
| ------- | ----- | ------- |
| `<keyframeTrack>` | `property` (**required**) | The prop of the holding element it drives, by name. One track per prop. |
| `<keyframe>` | `time` (**required**), `value` (**required**), `easing` | One keyframe: node-local time in any [time format](./timing.md#time-formats), the value at that time (a number, or any CSS color on a `color` track), and the easing into the next keyframe. |

Animatable props (`property`): `x`, `y`, `offsetX`, `offsetY`, `width`, `height`, `rotation`, `scale`, `scaleX`, `scaleY`, `opacity`, `cornerRadius`, `cornerRadiusTopLeft`, `cornerRadiusTopRight`, `cornerRadiusBottomRight`, `cornerRadiusBottomLeft`, `volume`, `color`, `offset`, `blur`, `value`. Whose prop is the track's holder's: a track under a [paint or color stop](./paints.md) animates the paint (`color`, `opacity`, `offset`), and one under a [`<stroke>`, `<shadow>` or `<effect>`](./styles.md) that style's own — `width` is a stroke's line width, `blur`/`offsetX`/`offsetY` a shadow's, `value` an effect's amount. For preset in/out effects (fade, slides, text reveals, ...) use [`<animation>`](./animations.md) instead.

## Semantics

- `time` is **node-local**: `0` is where the clip begins (its `start`). Timing props are parent-relative; keyframe times are not, so animation moves with the clip.
- Keyframes may be written in any order; they sort by `time`. Outside the keyframed range the value holds at the first/last keyframe.
- `easing` shapes the segment from its keyframe to the next; the last keyframe's easing is ignored. Default `"linear"`.
- The prop's static value (`x={10}` on the element) is what holds when the track has no keyframes; while it has any, the track wins.
- Tracks and keyframes are elements like any other: each has an `id`, the editor writes a moved keyframe back to it, and they are copied with their element.

## Easing

| Easing | Use for |
| ------ | ------- |
| `"linear"` (default) | Constant-rate change. |
| `"easeIn"`, `"easeOut"`, `"easeInOut"` | Standard acceleration curves (CSS equivalents). |
| `"gentle"`, `"snappy"`, `"bouncy"`, `"strong"` | Spring presets, from soft settle to hard overshoot. |
| `"cubicBezier(x1,y1,x2,y2)"` | Custom curve, CSS control points. |
| `"spring(bounce,duration)"` | Custom spring: bounce `0`-`1`, duration in ms. |
| `"steps(n)"` | Discrete hold: n equal steps, no interpolation. |

Named presets expand to the same descriptors the editor's interpolation inspector writes, so they round-trip cleanly.
