export default <stage background="#0b101b">
  <scene id="paints" active width={1024} height={576} fill="#0b101b" workarea={[0,2]}>
    <rect x={40} y={36} width={944} height={504} fill="#162238" cornerRadius={28}>
      <radialGradientPaint opacity={0.4}>
        <colorStop color="#346998" offset={0}/>
        <colorStop color="#162238" offset={1}/>
      </radialGradientPaint>
      <stroke color="#35506b" width={2}/>
    </rect>
    <text x={78} y={64} color="#94abc3" fontSize={16} letterSpacing={3}>AD3 / NATIVE COMPOSITOR</text>
    <text x={76} y={124} width={880} height={86} fontSize={72} fontWeight={800}>
      Gradientes em Rust.
      <stroke color="#0c1729" width={5} join="round"/>
      <linearGradientPaint>
        <colorStop color="#7cf5ce" offset={0}/>
        <colorStop color="#84a3ff" offset={1}/>
      </linearGradientPaint>
    </text>
    <text x={80} y={232} width={840} height={56} color="#c0d0e2" fontSize={24}>Preenchimentos, contornos e modos de mistura animados.</text>
    <rect x={80} y={326} width={264} height={126} cornerRadius={20}>
      <linearGradientPaint rotation={0}>
        <colorStop color="#3fe1bb" offset={0}/>
        <colorStop color="#6f65f1" offset={1}/>
        <keyframeTrack property="rotation"><keyframe value={0}/><keyframe time={2} value={130}/></keyframeTrack>
      </linearGradientPaint>
      <stroke color="#bafce8" width={2} opacity={0.6}/>
    </rect>
    <rect x={380} y={326} width={264} height={126} cornerRadius={20}>
      <radialGradientPaint x={0.3} y={0.4}>
        <colorStop color="#ffdeb0" offset={0}/>
        <colorStop color="#e865a5" offset={0.5}/>
        <colorStop color="#6545b4" offset={1}/>
        <keyframeTrack property="x"><keyframe value={0.3}/><keyframe time={2} value={0.8}/></keyframeTrack>
      </radialGradientPaint>
      <stroke color="#f6bfdc" width={2} opacity={0.6}/>
    </rect>
    <rect x={680} y={326} width={264} height={126} cornerRadius={20} fill="#12213c">
      <stroke color="#5f84bd" width={2}/>
    </rect>
    <group x={718} y={347} blendMode="screen">
      <rect width={112} height={84} cornerRadius={42} fill="#258ccb"/>
      <rect x={68} width={112} height={84} cornerRadius={42} fill="#dc478a"/>
    </group>
    <text x={80} y={477} width={264} height={30} fontSize={18} color="#9eb8d4" textAlign="center">LINEAR</text>
    <text x={380} y={477} width={264} height={30} fontSize={18} color="#9eb8d4" textAlign="center">RADIAL</text>
    <text x={680} y={477} width={264} height={30} fontSize={18} color="#9eb8d4" textAlign="center">SCREEN</text>
  </scene>
</stage>;
