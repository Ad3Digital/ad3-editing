// Rust samples presets/keyframes from this source for every preview/export frame.
export default <stage>
  <scene id="main" active width={960} height={540} fill="#12151E" workarea={[0,4]}>
    <rect id="card" x={80} y={100} width={800} height={320} fill="#3C6EF0" end={4}>
      <animation type="slideLeft" duration={0.8}/>
      <animation type="fade" phase="out" duration={0.6}/>
      <keyframeTrack property="color">
        <keyframe time={0} value="#3C6EF0" easing="easeInOut"/>
        <keyframe time={2} value="#AD47DF" easing="easeInOut"/>
        <keyframe time={4} value="#3C6EF0"/>
      </keyframeTrack>
    </rect>
    <text id="title" x={130} y={175} fontFamily="Arial" fontSize={58} color="white" end={4}>
      Movimento em Rust
      <animation type="appearChar" duration={1.5}/>
      <animation type="fade" phase="out" duration={0.6}/>
    </text>
    <text id="subtitle" x={135} y={265} fontFamily="Arial" fontSize={26} color="white" start={1.4} end={4}>
      Presets, cores e keyframes na mesma composição.
      <animation type="fade" duration={0.5}/>
      <animation type="fade" phase="out" duration={0.6}/>
    </text>
  </scene>
</stage>;
