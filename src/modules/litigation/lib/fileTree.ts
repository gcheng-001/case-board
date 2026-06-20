/**
 * 源文件看板重构 · Phase 2:从 `document.source_path` 相对 `case.source_folder`
 * **派生**原始文件夹树(任意深度、零 schema、纯前端)。
 * 只读派生,不落库——刷新/重抽后自动跟着 source_path 走。
 */
import type { Document } from "@/lib/types";

export interface FileTreeNode {
  /** 文件夹名(根节点为空串) */
  name: string;
  /** 子文件夹(已按名排序) */
  folders: FileTreeNode[];
  /** 本层直接文件(已按名排序) */
  files: Document[];
}

/** 把绝对路径 `abs` 转成相对 `base` 的路径(分隔符无关;不在 base 下则退回文件名)。 */
function relativeTo(abs: string, base: string): string {
  const a = abs.replace(/\\/g, "/");
  const b = base.replace(/\\/g, "/").replace(/\/+$/, "");
  if (b && (a === b || a.startsWith(b + "/"))) {
    return a.slice(b.length + 1);
  }
  // 不在案件目录下(共用材料 / 合并进来的) → 只取文件名,挂到根
  return a.split("/").pop() ?? a;
}

/** 构建文件夹树(根节点的 name 为空串)。 */
export function buildFileTree(
  docs: Document[],
  sourceFolder: string,
): FileTreeNode {
  const root: FileTreeNode = { name: "", folders: [], files: [] };
  for (const doc of docs) {
    const rel = relativeTo(doc.source_path, sourceFolder);
    const segs = rel.split("/").filter(Boolean);
    segs.pop(); // 末段是文件名,丢掉只留文件夹路径
    let node = root;
    for (const seg of segs) {
      let child = node.folders.find((f) => f.name === seg);
      if (!child) {
        child = { name: seg, folders: [], files: [] };
        node.folders.push(child);
      }
      node = child;
    }
    node.files.push(doc);
  }
  const sortRec = (n: FileTreeNode) => {
    n.folders.sort((a, b) => a.name.localeCompare(b.name, "zh"));
    n.files.sort((a, b) => a.filename.localeCompare(b.filename, "zh"));
    n.folders.forEach(sortRec);
  };
  sortRec(root);
  return root;
}

/** 递归数一个节点(含所有子文件夹)下的文件总数。 */
export function countFiles(node: FileTreeNode): number {
  return node.files.length + node.folders.reduce((s, f) => s + countFiles(f), 0);
}

/** 递归收集一个节点(含所有子文件夹)下的全部文档(文件夹级批量标记用)。 */
export function collectDocs(node: FileTreeNode): Document[] {
  return [...node.files, ...node.folders.flatMap(collectDocs)];
}
