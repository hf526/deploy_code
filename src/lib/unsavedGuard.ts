/** 未保存修改守卫：当前页面（仓库工作区）注册拦截器，全局跳转入口统一走 runGuarded。 */
type UnsavedGuard = (action: () => void) => void;

let currentGuard: UnsavedGuard | null = null;

export function registerUnsavedGuard(guard: UnsavedGuard | null) {
  currentGuard = guard;
}

/**
 * 经过未保存守卫执行导航等动作：有未保存修改时先弹确认，
 * 确认后再执行；没有注册守卫时直接执行。
 */
export function runGuarded(action: () => void) {
  if (currentGuard) currentGuard(action);
  else action();
}
