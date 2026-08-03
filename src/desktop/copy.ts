import type { Lang } from '../app';

/**
 * 桌面外壳独有的文案。
 *
 * 刻意不并进 `app.tsx` 的 `copy`：那份是网页版的，改它就等于改网页版。
 * 桌面版多出「模板库」「近期任务」等概念，网页版没有。
 */
export const desktopCopy = {
  zh: {
    nav: { paper: '科研图', ppt: '幻灯片', templates: '模板库', settings: '设置' },
    recent: { title: '近期任务', empty: '还没有任务', failed: '失败', running: '进行中', done: '完成' },
    templates: {
      title: '模板库',
      intro: '科研图以模板作 few-shot 参考。可单张导入，也可导入整个 PaperBananaBench 目录。',
      importImage: '导入图片',
      importPack: '导入模板包',
      packHint: '选择解压后的 PaperBananaBench 目录（内含 diagram/ 与 plot/ 子目录及 ref.json）',
      pickPack: '选择目录…',
      kind: '类型',
      category: '分类',
      categoryHint: '可选，如 architecture / bar chart',
      visualIntent: '视觉意图',
      visualIntentHint: '可选，这张图想表达什么',
      contentSummary: '内容描述',
      contentSummaryHint: '可选，图里画了什么',
      file: '模板图片',
      choose: '选择图片',
      submit: '导入',
      importing: '导入中…',
      imported: '已导入',
      importedPack: (count: number) => `已导入 ${count} 张模板`,
      needFile: '请先选择图片',
      search: '搜索',
      all: '全部',
      diagram: '示意图',
      plot: '图表',
      count: (n: number) => `共 ${n} 张`,
      emptyTitle: '模板库还是空的',
      emptyBody: '科研图模式需要参考图库。下载 PaperBananaBench 解压后，用上面的「导入模板包」选择该目录；或先单张导入几张自有模板。',
      emptyLink: '下载 PaperBananaBench'
    }
  },
  en: {
    nav: { paper: 'Figure', ppt: 'Slide', templates: 'Templates', settings: 'Settings' },
    recent: { title: 'Recent jobs', empty: 'No jobs yet', failed: 'failed', running: 'running', done: 'done' },
    templates: {
      title: 'Template library',
      intro: 'Paper figures use templates as few-shot references. Import single images, or a whole PaperBananaBench directory.',
      importImage: 'Import image',
      importPack: 'Import pack',
      packHint: 'Pick the extracted PaperBananaBench directory (containing diagram/ and plot/ with ref.json)',
      pickPack: 'Choose directory…',
      kind: 'Kind',
      category: 'Category',
      categoryHint: 'Optional, e.g. architecture / bar chart',
      visualIntent: 'Visual intent',
      visualIntentHint: 'Optional, what the figure conveys',
      contentSummary: 'Content summary',
      contentSummaryHint: 'Optional, what is drawn',
      file: 'Template image',
      choose: 'Choose image',
      submit: 'Import',
      importing: 'Importing…',
      imported: 'Imported',
      importedPack: (count: number) => `Imported ${count} templates`,
      needFile: 'Choose an image first',
      search: 'Search',
      all: 'All',
      diagram: 'Diagram',
      plot: 'Plot',
      count: (n: number) => `${n} total`,
      emptyTitle: 'No templates yet',
      emptyBody:
        'Paper figure mode needs a reference library. Download and extract PaperBananaBench, then use "Import pack" above to select that directory — or import a few of your own templates individually.',
      emptyLink: 'Download PaperBananaBench'
    }
  }
} as const;

export type DesktopCopy = (typeof desktopCopy)[Lang];

export const PAPER_BANANA_BENCH_URL = 'https://huggingface.co/datasets/dwzhu/PaperBananaBench';
