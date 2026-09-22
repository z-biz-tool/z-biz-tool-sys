import { useCallback, useMemo, useState } from "react";
import type { ProcessQuery } from "../ipc_contract";
import type { ProcessSortState } from "../components/tabs/ProcessTab";
import { PROCESS_PAGE_SIZE, PROCESS_PAGE_SIZE_OPTIONS } from "../lib/ui";

/**
 * 进程表查询条件的单点归属（T4-03）。方案里这一项写的是"新建 `useProcessStore.ts`（Zustand）"，
 * 实测这里没有跨层共享需求：条件只在 `App` 组合成 `ProcessQuery` 交给 `useProcessStream`，
 * `ProcessTab` 全程受控 props。为一个消费者引状态库只会多一条要手工保持同步的链路。
 */
export function useProcessQuery() {
  const [keyword, setKeyword] = useState("");
  const [sort, setSort] = useState<ProcessSortState>({ by: "cpu", desc: true });
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(PROCESS_PAGE_SIZE);

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
  /**
   * 页容量只在白名单里取，并且同样回到第一页 —— 从第 6 页 ×20 行切到 ×200 行时，
   * 原来的偏移会落到新结果集的空段上。
   */
  const changePageSize = useCallback((next: number) => {
    if (!PROCESS_PAGE_SIZE_OPTIONS.includes(next)) return;
    setPageSize(next);
    setPage(1);
  }, []);
  /**
   * 页码不能越过当前结果集的页数。T5-10 之后这一步更要紧：换页容量、或进程数在翻页途中缩水时，
   * 旧偏移会落到新结果集末尾之后，而后端 `offset.min(total)` 只会给出**空的一页** —— 表格看着像"没进程"。
   * 由 `App` 拿每帧的 `total` 调一次，越界才改状态（不越界时不产生新的 query 引用）。
   */
  const correctPageToRange = useCallback(
    (total: number) => {
      const maxPage = Math.max(1, Math.ceil(total / pageSize));
      setPage((current) => (current > maxPage ? maxPage : current));
    },
    [pageSize]
  );

  const query = useMemo<ProcessQuery>(
    () => ({
      keyword: keyword.trim().toLowerCase(),
      sortBy: sort.by,
      desc: sort.desc,
      offset: (page - 1) * pageSize,
      limit: pageSize,
    }),
    [keyword, sort, page, pageSize]
  );

  return {
    query,
    keyword,
    sort,
    page,
    pageSize,
    changeKeyword,
    changeSort,
    changePage,
    changePageSize,
    correctPageToRange,
  };
}
