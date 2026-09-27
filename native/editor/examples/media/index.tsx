export default <stage background="#10151f">
  <scene id="media" active width={960} height={540} fill="#10151f" workarea={[0, 2]} cornerRadius={28}>
    <rect x={40} y={40} width={530} height={360} fill="#27344a" cornerRadius={24}/>
    <video id="video" src="pattern.mp4" x={70} y={70} width={470} height={300}
      cornerRadius={24} objectFit="cover" sourceIn={0.25} sourceOut={2.75} playbackRate={1.25}>
      <keyframeTrack property="rotation"><keyframe value={-5}/><keyframe time={2.7} value={5}/></keyframeTrack>
    </video>
    <group x={650} y={70} rotation={10} opacity={0.85}>
      <rect width={220} height={300} fill="#354a6e" cornerRadius={28}/>
      <image id="contain" src="tiles.png" x={15} y={15} width={190} height={270}
        objectFit="contain" cornerRadius={24}/>
    </group>
    <image id="fill" src="tiles.png" x={600} y={405} width={275} height={75}
      objectFit="fill" cornerRadiusTopLeft={30} cornerRadiusBottomRight={30}/>
    <rect x={75} y={440} width={440} height={12} fill="#3adeae" cornerRadius={6}>
      <keyframeTrack property="width"><keyframe value={40}/><keyframe time={2} value={440}/></keyframeTrack>
    </rect>
  </scene>
</stage>;
