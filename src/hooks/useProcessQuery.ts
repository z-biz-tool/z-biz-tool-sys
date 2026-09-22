import { useCallback, useMemo, useState } from "react";
import type { ProcessQuery } from "../ipc_contract";
import type { ProcessSortState } from "../components/tabs/ProcessTab";
import { PROCESS_PAGE_SIZE } from "../lib/ui";

/**
 * 进程表查询条件的单点归属（T4-03）。方案里这一项写的是"新建 `useProcessStore.ts`（Zustand）"，
 * 实测这里没有跨层共享需求：条件只在 `App` 组合成 `ProcessQuery` 交给 `useProcessStream`，
 * `ProcessTab` 全程受控 props。为一个消费者引状态库只会多一条要手工保持同步的链路。
 */
export function useProcessQuery() {
  const [keyword, setKeyword] = useState("");
  const [sort, setSort] = useState<ProcessSortState>({ by: "cpu", desc: true });
  const [page, setPage] = useState(1);

  // 关键字或排序变化后回到第一页：否则偏移会落在新结果集的空段上。
  const changeKeyword = useCallback((value: string) => {
    setKeyword(value);
    setPage(1);
  }, []);
  const changeSort = useCallback((next: ProcessSortState) => {
    setSort(next);
    setPage(1);
  }, []);
  const changePage = useCallback((next: number) => {
    setPage(Math.max(1, next));
  }, []);

  const query = useMemo<ProcessQuery>(
    () => ({
      keyword: keyword.trim().toLowerCase(),
      sortBy: sort.by,
      desc: sort.desc,
      offset: (page - 1) * PROCESS_PAGE_SIZE,
      limit: PROCESS_PAGE_SIZE,
    }),
    [keyword, sort, page]
  );

  return { query, keyword, sort, page, changeKeyword, changeSort, changePage };
}
