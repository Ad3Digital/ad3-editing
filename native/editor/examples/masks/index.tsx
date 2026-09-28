export default <stage background="#111923">
  <scene id="masks" active width={960} height={540} workarea={[0, 2]} fill="#111923">
    <text x={54} y={38} width={860} height={70} fontSize={43} fontWeight={700} color="white">MASKS / NATIVE RUST</text>
    <text x={56} y={108} width={840} height={42} fontSize={20} color="#acbdce">Rounded windows, intersecting clips and animated reveals.</text>
    <rect x={54} y={182} width={398} height={250} cornerRadius={24}>
      <rect mask width={398} height={250} cornerRadius={24}/>
      <linearGradientPaint rotation={25}>
        <colorStop color="#2ff1b2" offset={0}/>
        <colorStop color="#147ab6" offset={1}/>
      </linearGradientPaint>
      <rect x={-70} y={74} width={560} height={84} rotation={-18} fill="#edffcf"/>
      <text x={28} y={86} width={350} height={80} fontSize={61} fontWeight={800} color="#0b3944">CLIPPED</text>
    </rect>
    <rect x={502} y={182} width={404} height={250}>
      <rect mask width={404} height={250} cornerRadius={24}/>
      <rect mask hidden x={0} y={0} width={350} height={250} rotation={-9}/>
      <linearGradientPaint rotation={-25}>
        <colorStop color="#ff835d" offset={0}/>
        <colorStop color="#a650d9" offset={1}/>
      </linearGradientPaint>
      <text x={28} y={90} width={350} height={74} fontSize={50} fontWeight={800} color="white">
        REVEAL
        <rect mask width={360} height={90}>
          <keyframeTrack property="width"><keyframe value={40}/><keyframe time={1.5} value={360}/></keyframeTrack>
        </rect>
      </text>
    </rect>
    <text x={56} y={469} width={840} height={32} fontSize={19} color="#acbdce">One Rust compositor for preview, PNG capture and video export.</text>
  </scene>
</stage>;
