import { Card } from './card';

export default () => <stage>
  <scene id="main" active width={960} height={540} workarea={[0, 5]}>
    <rect id="background" width={960} height={540} fill="#101720" />
    <Card />
  </scene>
</stage>;
