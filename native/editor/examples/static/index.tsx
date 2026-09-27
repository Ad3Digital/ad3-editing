export default function Project() {
  return (
    <stage>
      <scene id="main" name="Native editing check" active
        width={640} height={360} fill="#171c27" workarea={[0, 3]}>
        <rect id="card" name="Teal card" x={72} y={120}
          width={496} height={160} fill="#26bca2" start={0} end={3} />
        <text id="title" name="Title" x={72} y={52}
          fontFamily="Arial" fontSize={32} color="white" start={0} end={3}>
          AD3 Editing
        </text>
      </scene>
    </stage>
  );
}
