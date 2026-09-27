export default <stage background="#10151f">
  <scene id="vectors" active width={960} height={540} fill="#10151f" workarea={[0, 3]} cornerRadius={32}>
    <rect x={80} y={80} width={240} height={160} fill="#fb703e" cornerRadius={32} rotation={-12}>
      <animation type="grow" duration={0.8}/>
    </rect>
    <rect x={380} y={120} width={220} height={220} fill="#3adeae" cornerRadius={110} scaleX={1.2}/>
    <group x={670} y={110} opacity={0.65} rotation={12}>
      <rect width={130} height={180} fill="#5994f5" cornerRadius={24}/>
      <rect x={48} y={70} width={130} height={180} fill="#f0c950" cornerRadiusTopLeft={48} cornerRadiusBottomRight={48}/>
    </group>
    <rect x={110} y={380} width={440} height={64} fill="#eee8dc" cornerRadius={20}>
      <keyframeTrack property="rotation"><keyframe value={-4}/><keyframe time={2.9} value={4}/></keyframeTrack>
    </rect>
  </scene>
</stage>;
