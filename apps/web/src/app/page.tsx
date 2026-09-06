import { AccountPanel } from "@/components/account-panel";

const modules = [
  {
    eyebrow: "KNOWLEDGE",
    title: "个人知识库",
    copy: "导入 Markdown、PDF 与网页，建立可检索的长期知识。",
    status: "已支持 Markdown 导入",
  },
  {
    eyebrow: "TODAY",
    title: "今日信息",
    copy: "聚合高价值信息，并把噪音留在视野之外。",
    status: "尚未启用",
  },
  {
    eyebrow: "LEARNING",
    title: "学习任务",
    copy: "让技能图谱转化为今天可以完成的一小步。",
    status: "尚未启用",
  },
];

export default function Home() {
  return (
    <main>
      <nav aria-label="Primary navigation">
        <a className="brand" href="#top" aria-label="Personal AI home">
          <span className="brandMark">P</span>
          <span>PERSONAL AI</span>
        </a>
        <span className="environment">LOCAL / PRIVATE</span>
      </nav>

      <section className="hero" id="top">
        <p className="kicker">A QUIET SYSTEM FOR COMPOUNDING KNOWLEDGE</p>
        <h1>把信息变成你的<br />长期能力。</h1>
        <p className="intro">
          一个属于个人的 AI 工作台：持续积累知识、筛选信息、规划学习，并为未来的 Agent 工作流保留清晰边界。
        </p>
        <form className="prompt">
          <label htmlFor="prompt">ASK YOUR KNOWLEDGE</label>
          <div>
            <input id="prompt" name="prompt" placeholder="从一个问题开始……" disabled />
            <button type="button" disabled aria-label="Send prompt">→</button>
          </div>
          <small>AI Chat 将在 Knowledge Engine 接入后启用</small>
        </form>
      </section>

      <AccountPanel />
      <section className="moduleGrid" aria-label="System modules">
        {modules.map((module, index) => (
          <article key={module.eyebrow}>
            <div className="cardNumber">0{index + 1}</div>
            <p>{module.eyebrow}</p>
            <h2>{module.title}</h2>
            <p className="cardCopy">{module.copy}</p>
            <span className="status"><i />{module.status}</span>
          </article>
        ))}
      </section>

      <footer>
        <span>FOUNDATION / v0.1.0</span>
        <span>DATA STAYS UNDER YOUR CONTROL</span>
      </footer>
    </main>
  );
}
