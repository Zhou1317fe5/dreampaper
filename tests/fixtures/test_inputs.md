# dreampaper 手测输入

## 启动地址

- 前端: http://127.0.0.1:5173/
- 后端: http://127.0.0.1:8000/api/health

## Model 配置提示

1. 打开左侧 **Model**
2. Design: 任选 `openai_chat` / `openai_responses` / `anthropic_messages`，填 base_url / model / api_key
3. Implement: 选 `image2` 或 `banana2`，填输出尺寸等
4. 代理默认 `http://127.0.0.1:7890`，不用则清空
5. 保存后再测 Figure / Slide

---

## 用例 A — Paper Figure（Transformer 架构图）

**模式**: Figure / diagram  
**标题**:
```
Overall architecture of the Transformer
```

**方法描述**（粘贴到「方法」框）:
```
The Transformer is a sequence transduction model based entirely on attention, without recurrence or convolution. Input tokens are converted to embeddings and added with sinusoidal positional encodings. The encoder is a stack of N identical layers; each layer has multi-head self-attention and a position-wise feed-forward network, each wrapped by residual connection and layer normalization. The decoder is also a stack of N layers; each layer has masked multi-head self-attention, multi-head cross-attention over encoder outputs, and a feed-forward network, again with residual connections and layer normalization. Scaled dot-product multi-head attention computes relevance among queries, keys, and values in multiple subspaces. The decoder output is projected by a linear layer and softmax to predict the next token. Draw a clean academic pipeline figure with encoder stack on the left, decoder stack on the right, clear arrows for residual paths, and short English labels only.
```

**建议 template**: kind=diagram，选 1 张 pipeline 类参考图  
**比例**: 16:9 或 inherit  
**布局**: balanced  

---

## 用例 B — Paper Figure（LoRA 适配图）

**标题**:
```
LoRA low-rank adaptation for frozen Transformers
```

**方法描述**:
```
LoRA freezes the pre-trained weight matrix W0 of a large language model and injects a trainable low-rank update Delta W = B A, where A is a down-projection to rank r and B is an up-projection back to the original dimension, with r much smaller than d. Only A and B are optimized for the downstream task while W0 stays frozen, greatly reducing trainable parameters and memory. During the forward pass, the layer computes W0 x + (alpha/r) B A x. At inference the low-rank factors can be merged into W0 so there is no extra latency. Visualize one Transformer weight path with a frozen backbone branch and a parallel LoRA adapter branch (A then B), merge/add node, and labels for frozen vs trainable parameters.
```

**建议 template**: diagram，选 adapter/module 风格参考  

---

## 用例 C — Paper Figure（RankRAG 流程）

**标题**:
```
RankRAG retrieve-rerank-generate pipeline
```

**方法描述**:
```
RankRAG unifies context ranking and answer generation in one instruction-tuned LLM for retrieval-augmented generation. At inference, an external retriever first fetches a large top-N candidate set for the query. The same LLM then acts as a reranker via instructions to select a high-quality top-k subset. Finally the LLM generates the answer conditioned on the query and the refined contexts. Training has two stages: Stage I supervised fine-tuning on general instruction data; Stage II continues with a mix of instruction data, context-rich QA, retrieval-augmented QA, and ranking tasks so relevance detection and answer extraction reinforce each other. Draw a left-to-right system pipeline: Query -> Retriever top-N -> LLM Rerank top-k -> LLM Generate answer, plus a small two-stage training inset if space allows.
```

---

## 用例 D — PPT Slide（5 页，RankRAG 短讲）

**模式**: Slide  
**template**: 上传 `tests/fixtures/ppt_template_academic.png`  
**页数**: 5  

**资料文本**:
```
Paper: RankRAG: Unifying Context Ranking with Retrieval-Augmented Generation in LLMs (NeurIPS 2024)

Talk goal: 8-minute oral-style overview for an academic audience.

Page plan (system should auto-pick A/B/C skeletons):
1) Cover — Title RankRAG; authors placeholder "Research Team"; venue NeurIPS 2024; one-line tagline: one LLM for ranking and generation in RAG.
2) Problem — Standard RAG uses retrieve-then-generate; fixed small top-k trades recall vs noise; separate rankers lack LLM generalization; need unified ranking + answering.
3) Method — External retriever returns top-N; same LLM reranks to top-k by instruction; same LLM generates the answer; two-stage training: general SFT then mixed ranking + RAG QA data.
4) Experiments — Knowledge-intensive QA benchmarks; compare against retrieve-then-generate and pipeline rankers; metrics: accuracy / EM / F1 style; highlight gains from unified rerank+generate.
5) Conclusion — One model can both select context and answer; improves RAG precision without a separate ranker at deploy time; limitations: depends on retriever quality and instruction format; future: multi-domain and longer contexts.

Style constraints:
- Academic red-gray on pure white
- 16:9
- Short labels, no dense paragraphs
- Keep master title bar, divider, page number, logo region from the uploaded template
- English titles, bilingual short bullets OK (EN primary)
```

---

## 用例 E — PPT Slide（6 页，Transformer 方法讲）

**页数**: 6  
**资料文本**:
```
Paper: Attention Is All You Need (NeurIPS 2017)

Need a 3-page academic slide deck:

1. Cover: Attention Is All You Need — The Transformer; Vaswani et al.; NeurIPS 2017.
2. Motivation: RNNs/CNNs limit parallelization and long-range dependency modeling for sequence transduction.
3. Architecture overview: encoder-decoder stacks of N layers; pure attention; positional encodings; no recurrence.
4. Multi-head attention: scaled dot-product; Q/K/V projections; multiple heads; residual + LayerNorm; FFN.
5. Results theme: WMT translation quality with more parallel training; BLEU improvements vs prior seq2seq; training efficiency narrative (qualitative, no fabricated exact numbers).
6. Takeaways: attention-only sequence models became the foundation for modern LLMs; limitations of original work (fixed context, quadratic attention) and later extensions.

Visual emphasis: architecture block diagrams on pages 3-4; avoid fake tables with invented metrics.
```

---

## 用例 F — PPT Slide（中文实物素材，重点验证 websearch）

**模式**: Slide
**template**: 上传 `tests/fixtures/ppt_template_academic.png`
**页数**: 5

> 本用例专门验证中文实物抽词与真实素材呈现。预期抽出 8 个主体，
> 页面上应画出仪器/设备本身，而不是写着名字的方框。

**资料文本**:
```
课题：面向近海水体的多源污染物快速筛查方法

研究背景：近海养殖区抗生素与微塑料残留检测周期长，现场缺乏快速筛查手段。

技术路线：
1. 野外采样。使用六旋翼无人机搭载采水装置完成多点位取样，样品现场装入冷藏箱运回。
2. 前处理。样品经离心机分离固液两相，上清液过滤后转入进样瓶，固相置于培养箱恒温暂存。
3. 仪器分析。采用高分辨质谱仪完成抗生素定量，配合色谱仪分离共流出物；微塑料形貌
   由扫描电镜表征，粒径分布用激光粒度仪统计。
4. 数据处理。谱图经基线校正后比对标准库，异常样本回到反应釜做加标回收验证。
5. 结果输出。生成污染物空间分布图与风险等级评估。

演示要求：
- 第 2、3 页需要能看出设备形态，读者应一眼认出是什么仪器
- 保持模板红灰配色与页码/分隔线
- 中文可见文字，短标签为主
```

**预期检查**：
- 后端事件出现 `ppt_visual_assets`，`internal_artifacts.visual_asset_context.terms` 包含无人机 / 离心机 / 质谱仪 / 扫描电镜等
- `degraded` 为 `false`（能搜到结果时）
- 页面 JSON 的 `visual_element_plan.elements[].appearance` 有具体形态描述，不是空话
- 出图中设备是画出来的实物轮廓，不是色块加文字

---

## 用例 G — PPT Slide（中英混合工程素材）

**页数**: 4

> 验证中英分流检索：中文主体查实物外观，英文品牌查 logo。

**资料文本**:
```
项目：边缘侧视觉质检系统落地方案

硬件构成：产线部署工业相机与激光雷达做定位，推理侧采用 NVIDIA Jetson 模组，
辅以树莓派完成信号采集，缺陷样品由机械臂分拣至对应料仓。

软件栈：模型基于 PyTorch 训练，容器化后用 Docker 打包，通过 Kubernetes 在边缘集群编排，
指标写入 PostgreSQL，看板用 Grafana 展示。

流程：图像采集 → 边缘推理 → 缺陷判定 → 机械臂分拣 → 数据回流复训

演示要求：硬件页要画出设备实物，软件页可用产品标识，保持模板配色。
```

**预期检查**：
- 中文词（机械臂/激光雷达/树莓派/模组）走 `实物外观` 检索
- 英文词（PyTorch/Docker/Kubernetes/Jetson）走 `official logo` 检索
- 硬件页画实物、软件页用标识，两种处理方式并存

---

## 用例 H — PPT Slide（纯抽象内容，负向用例）

**页数**: 3

> 验证抽不到实物时不硬凑视觉元素、不编造品牌标识。

**资料文本**:
```
课题：一类非凸优化问题的收敛性分析

内容：针对带非光滑正则项的复合优化问题，提出一种自适应步长的近端梯度算法。
通过构造 Lyapunov 函数证明算法在弱凸假设下收敛到稳定点，并给出 O(1/k) 的收敛速率。
进一步分析步长参数对收敛常数的影响，讨论与已有方法在理论保证上的差异。
```

**预期检查**：
- `visual_asset_context.terms` 为空，`degraded` 为 `true`
- 不发起任何检索请求（日志中无搜索耗时）
- 页面用公式排版/示意曲线，`usage_decision` 明确说明为何不用实物图
- **不应**出现凭空捏造的仪器图或品牌标识

---

## 手测检查清单

- [ ] Model 配置保存后刷新仍在（API key 脱敏）
- [ ] Figure 任务进度事件可见，最终出图或错误可读
- [ ] Slide 上传 template + 资料后按页出图
- [ ] 代理不通时错误是否可诊断
- [ ] 任务产物是否落在 ~/.dreampaper/jobs/
- [ ] 用例 F：中文实物词能抽出，出图画的是设备实物而非文字方框
- [ ] 用例 G：中英文检索词分流正确
- [ ] 用例 H：抽象内容不硬凑视觉元素、不编造标识
- [ ] 设置页折叠交互：展开/收起动画平滑，收起段无法被 Tab 选中
