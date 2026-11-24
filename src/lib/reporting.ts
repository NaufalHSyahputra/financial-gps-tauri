import type { Money, ReportPoint_Serialize, ReportKind, Totals_Serialize } from './drivers/runtime';

export type ChartPoint = {date:string;values:{label:string;money:Money}[]};

export function primary(point:ReportPoint_Serialize, kind:ReportKind):Totals_Serialize|null {
  if(kind==='netWorth') return point.netWorth;
  if(kind==='cashFlow') return point.forecast;
  return point.expense;
}

export function chartPoints(points:ReportPoint_Serialize[], kind:ReportKind):ChartPoint[] {
  return points.map(point=>{
    const series:[string,Totals_Serialize|null][] =
      kind==='netWorth' ? [['Net worth',point.netWorth]] :
      kind==='cashFlow' ? [['Booked',point.booked],['Forecast',point.forecast]] :
      kind==='incomeVsExpense' ? [['Income',point.income],['Expense',point.expense]] :
      [['Expense',point.expense]];
    return {date:point.date,values:series.flatMap(([label,total])=>total?[{label,money:total.reliable}]:[])};
  });
}
