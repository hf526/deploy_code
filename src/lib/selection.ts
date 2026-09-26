/**
 * 记录列表的多选状态。部署历史 / 备份 / Pages 三个列表共用同一套纯逻辑，
 * 页面里只负责渲染勾选框和调用批量删除命令。
 */

export type SelectedIds = ReadonlySet<string>;

/** 勾选或取消单个条目。 */
export function toggleId(current: SelectedIds, id: string, on: boolean): Set<string> {
  const next = new Set(current);
  if (on) {
    next.add(id);
  } else {
    next.delete(id);
  }
  return next;
}

/** 表头三态：候选为空与「一个都没选」同样返回 none，避免出现无意义的半选。 */
export function selectionState(
  selected: SelectedIds,
  candidates: Iterable<string>,
): "none" | "some" | "all" {
  let total = 0;
  let hit = 0;
  for (const id of candidates) {
    total += 1;
    if (selected.has(id)) hit += 1;
  }
  if (total === 0 || hit === 0) return "none";
  return hit === total ? "all" : "some";
}

/** 表头点击：已全选则取消这一批，否则把当前候选全部选上。 */
export function toggleAll(selected: SelectedIds, candidates: Iterable<string>): Set<string> {
  const list = [...candidates];
  const next = new Set(selected);
  if (selectionState(selected, list) === "all") {
    for (const id of list) next.delete(id);
  } else {
    for (const id of list) next.add(id);
  }
  return next;
}

/**
 * 丢掉已经不存在的 id。列表会被后台刷新或另一个窗口改变，
 * 留着失效条目会让「删除所选 (3)」指向已经不存在的记录。
 */
export function pruneSelection(
  selected: SelectedIds,
  candidates: Iterable<string>,
): Set<string> {
  const valid = new Set(candidates);
  const next = new Set<string>();
  for (const id of selected) {
    if (valid.has(id)) next.add(id);
  }
  return next;
}
