# Personal AI Augmentation System v1.0

# Agent Implementation Design Specification

## 1. 文档目标

本文档用于指导第一版 Personal AI Augmentation System 的工程落地。

目标：

-   可运行的个人 AI 平台
-   支持长期知识积累
-   支持 Agent 扩展
-   存储和模型可替换
-   为未来 MCP、多 Agent、个人工作流自动化预留能力

## 2. v1功能范围

第一版必须实现：

### Personal Dashboard

-   Web/PWA入口
-   AI Chat
-   今日信息
-   学习任务展示

### Knowledge Engine

能力：

-   Markdown/PDF/网页导入
-   文档解析
-   Chunk切分
-   Embedding
-   RAG检索
-   基于个人知识回答

### Information Agent

能力：

-   RSS采集
-   信息去重
-   AI价值评分
-   每日报告生成

### Learning Agent

能力：

-   Skill Graph
-   能力评估
-   学习计划生成
-   训练任务记录

### Agent Runtime

能力：

-   Context管理
-   Memory
-   Tool调用
-   Planner

------------------------------------------------------------------------

# 3. 总体架构

    User
     |
    Next.js PWA
     |
    API Gateway
     |
    Personal AI Runtime
     |
    +----------------+
    | Agent Runtime  |
    | Knowledge      |
    | Learning       |
    | Scheduler      |
    +----------------+
     |
    Storage Abstraction
     |
    +-----------+-----------+-----------+
    Postgres   VectorDB    ObjectStore

------------------------------------------------------------------------

# 4. 核心设计原则

## 4.1 外部依赖必须隔离

业务代码禁止直接依赖：

-   PostgreSQL
-   Qdrant
-   Redis
-   OpenAI

采用接口抽象。

例如：

``` rust
trait VectorStore {
    async fn insert();
    async fn search();
    async fn delete();
}
```

未来可以替换：

-   Qdrant
-   Milvus
-   PGVector
-   云向量服务

------------------------------------------------------------------------

# 5. Backend工程结构

Rust Workspace：

    personal-ai

    ├── apps
    │   ├── api-server
    │   ├── worker
    │   └── scheduler
    │
    ├── crates
    │   ├── domain
    │   ├── agent-core
    │   ├── knowledge
    │   ├── learning
    │   ├── storage
    │   ├── llm
    │   ├── tools
    │   └── mcp

------------------------------------------------------------------------

# 6. 存储设计

## PostgreSQL

保存：

-   User
-   Skill
-   Task
-   Agent状态
-   Metadata

## Vector Store

保存：

-   文档Embedding
-   知识检索索引

接口：

    VectorStore

    insert_embedding()

    similar_search()

    remove()

第一实现：

Qdrant。

## Object Storage

保存：

-   原始文件
-   大文本

接口：

    ObjectStorage

    put()

    get()

    delete()

第一实现：

MinIO。

兼容未来S3。

------------------------------------------------------------------------

# 7. LLM抽象

业务禁止直接调用模型。

接口：

    trait LLMProvider {

    chat()

    stream()

    embedding()

    }

实现：

-   OpenAI Provider
-   Local Model Provider
-   Other Cloud Provider

------------------------------------------------------------------------

# 8. Agent Runtime

v1采用单Agent。

流程：

    Input

    ↓

    Context Builder

    ↓

    Planner

    ↓

    Tool Executor

    ↓

    LLM

    ↓

    Memory Update

未来扩展：

    Supervisor Agent

    ├── Research Agent
    ├── Coding Agent
    ├── Learning Agent
    └── Market Agent

------------------------------------------------------------------------

# 9. Memory设计

三层Memory：

## Short Memory

当前对话。

实现：

Redis。

## Long Memory

用户事实：

例如：

-   技术背景
-   偏好
-   长期目标

实现：

Postgres。

## Semantic Memory

知识内容。

实现：

Vector DB。

------------------------------------------------------------------------

# 10. Knowledge Pipeline

    Import

    ↓

    Parser

    ↓

    Chunk

    ↓

    Embedding

    ↓

    Vector Store

    ↓

    RAG Retrieval

Metadata：

    source

    type

    created_time

    tags

------------------------------------------------------------------------

# 11. Tool系统

统一接口：

    trait Tool {

    name()

    description()

    execute()

    }

第一批：

-   KnowledgeSearch
-   WebSearch
-   FileReader
-   GitTool

未来：

通过 MCP Adapter 暴露。

------------------------------------------------------------------------

# 12. MCP扩展路线

    Internal Tool

    ↓

    MCP Adapter

    ↓

    MCP Server

    ↓

    External Agent

未来接入：

-   GitHub
-   Kubernetes
-   MongoDB
-   Pulsar
-   Market Data

------------------------------------------------------------------------

# 13. 部署方案

第一阶段：

Docker Compose。

组件：

    nginx

    web

    api-server

    worker

    scheduler

    postgres

    qdrant

    redis

    minio

未来迁移 Kubernetes。

------------------------------------------------------------------------

# 14. 开发路线

## Sprint 1

基础：

-   Rust Workspace
-   Next.js
-   PostgreSQL
-   用户系统

## Sprint 2

知识：

-   文档导入
-   RAG
-   AI问答

## Sprint 3

Agent：

-   Tool系统
-   Memory
-   Scheduler

## Sprint 4

增强：

-   Daily Brief
-   Skill Trainer

------------------------------------------------------------------------

# 15. 完成标准

第一版完成后：

每天可以：

1.  自动获取高价值信息
2.  基于个人知识回答问题
3.  生成学习任务
4.  保存长期记忆
5.  支持未来Agent扩展

最终目标：

构建个人长期AI基础设施。
