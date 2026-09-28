export default <stage background="#101b25">
  <scene id="effects" active width={640} height={360} workarea={[0, 1]} fill="#101b25">
    <text x={28} y={22} width={584} height={48} fontSize={30} fontWeight={750} color="white">LIGHT / COLOR / RUST</text>
    <text x={30} y={70} width={580} height={32} fontSize={15} color="#a9bfd0">Native shadows, ordered filters and animated color.</text>
    <rect x={30} y={128} width={174} height={116} cornerRadius={16} fill="#31e2ba">
      <shadow color="#02070c" blur={10} offsetX={5.5} offsetY={9.25} opacity={0.8}/>
      <text x={18} y={25} width={138} height={60} fontSize={42} fontWeight={800} color="#123b45">DEPTH<shadow color="#ffffff" blur={1} offsetX={1} offsetY={2} opacity={0.3}/></text>
    </rect>
    <rect x={234} y={128} width={174} height={116} cornerRadius={16}>
      <effect type="hueRotate" value={0}>
        <keyframeTrack property="value"><keyframe value={0}/><keyframe time={1} value={130}/></keyframeTrack>
      </effect>
      <linearGradientPaint rotation={25}><colorStop color="#ffbc60" offset={0}/><colorStop color="#e35985" offset={1}/></linearGradientPaint>
      <text x={17} y={27} width={142} height={60} fontSize={38} fontWeight={800} color="white">COLOR</text>
    </rect>
    <rect x={438} y={128} width={174} height={116} cornerRadius={16} fill="#577bf4">
      <rect mask width={174} height={116} cornerRadius={16}/>
      <rect x={-28} y={22} width={224} height={70} rotation={-18} fill="#ff71b3" blur={5}/>
      <text x={21} y={30} width={134} height={56} fontSize={36} fontWeight={800} color="white">BLUR</text>
    </rect>
    <text x={32} y={273} width={176} height={25} fontSize={14} color="#a9bfd0">Fractional shadow offset</text>
    <text x={236} y={273} width={180} height={25} fontSize={14} color="#a9bfd0">Animated hue rotation</text>
    <text x={440} y={273} width={172} height={25} fontSize={14} color="#a9bfd0">Blur inside a mask</text>
    <text x={32} y={320} width={572} height={24} fontSize={13} color="#7f98aa">One compositor for preview, PNG capture and video export.</text>
  </scene>
</stage>;
