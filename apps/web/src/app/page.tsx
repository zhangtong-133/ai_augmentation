import { AccountPanel } from "@/components/account-panel";

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
          一个属于个人的 AI 工作台：持续积累知识、筛选信息、规划学习，按需执行经你确认的 Agent 工作流。
        </p>
      </section>

      <AccountPanel />

      <footer>
        <span>FOUNDATION / v0.1.0</span>
        <span>DATA STAYS UNDER YOUR CONTROL</span>
      </footer>
    </main>
  );
}
