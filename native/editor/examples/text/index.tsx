// Inter ships with the native editor. This composition needs no media assets.
export default <stage background="#10141b">
  <scene id="text" active width={960} height={540} fill="#10141b" workarea={[0, 2]}>
    <rect x={48} y={44} width={864} height={452} fill="#192332" cornerRadius={28}/>
    <text x={84} y={70} color="#7d9fb9" fontSize={18} letterSpacing={3}>AD3 / NATIVE EDITOR</text>
    <text x={80} y={126} width={720} height={144} color="#f4f7fb" fontSize={68} fontWeight={700} leading={0.92}>
      Texto em Rust. Mesma composição.
      <textRange start={9} end={13} color="#49e0bd"/>
    </text>
    <text x={86} y={320} width={540} height={100} color="#b4c7dc" fontSize={26} leading={1.15}>
      Fontes, estilos e quebras de linha na prévia e no vídeo exportado.
      <textRange start={0} end={6} fontStyle="italic" color="#ffffff"/>
    </text>
    <group x={688} y={352} rotation={-8}>
      <rect width={170} height={72} fill="#49e0bd" cornerRadius={16}/>
      <text width={170} height={72} textAlign="center" textBaseline="middle" fontSize={24} fontWeight={700} color="#10141b">AO VIVO</text>
      <keyframeTrack property="rotation"><keyframe value={-8}/><keyframe time={2} value={8}/></keyframeTrack>
    </group>
    <rect x={86} y={457} width={540} height={4} fill="#49e0bd" cornerRadius={2}>
      <keyframeTrack property="width"><keyframe value={80}/><keyframe time={2} value={540}/></keyframeTrack>
    </rect>
  </scene>
</stage>;
