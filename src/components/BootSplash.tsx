import { Icon } from "./Icons";

/** 启动屏：暗色品牌底 + 脉冲环 logo + 胶片扫光加载条（影=胶片），阶段文案绑真实加载里程碑 */
export function BootSplash({ stage, fading }: { stage: number; fading: boolean }) {
  const STAGES = ["打开影匣…", "读取库元数据…", "加载影片墙…", "就绪"];
  const pct = [18, 45, 78, 100][Math.min(stage, 3)];
  return (
    <div className={`boot-splash ${fading ? "fade" : ""}`}>
      <div className="boot-mark">
        <span className="boot-ring r1" />
        <span className="boot-ring r2" />
        <div className="boot-logo"><Icon name="vault" size={34} /></div>
      </div>
      <div className="boot-title">影 匣</div>
      <div className="boot-film">
        <div className="boot-film-strip">
          {Array.from({ length: 8 }, (_, i) => <span key={i} className="boot-frame" />)}
          <span className="boot-sweep" />
        </div>
        <div className="boot-bar"><span style={{ width: `${pct}%` }} /></div>
      </div>
      <div className="boot-stage">
        {STAGES[Math.min(stage, 3)]}
        <span className="boot-dots" />
      </div>
      <div className="boot-foot">本地数据 · 零联网</div>
    </div>
  );
}
