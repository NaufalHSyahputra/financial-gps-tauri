import type { ReportKind, SearchKind } from './drivers/runtime';

export const reports: {kind:ReportKind;label:string;path:string}[] = [
  {kind:'netWorth',label:'Net worth',path:'net-worth'},
  {kind:'incomeVsExpense',label:'Income vs. expense',path:'income-expense'},
  {kind:'cashFlow',label:'Cash flow',path:'cash-flow'},
  {kind:'categoryTrends',label:'Category trends',path:'category-trends'}
];
export const navigation = [
  {label:'Overview',items:[['Dashboard','/dashboard'],['Transactions','/transactions'],['Accounts','/accounts'],['Calendar','/calendar'],['Reports','/reports/net-worth']]},
  {label:'Organize',items:[['Rules','/rules'],['Categories','/categories'],['People & stores','/payees']]},
  {label:'Plan',items:[['Cash plan','/planning'],['Budgets','/budgets'],['Recurring','/recurring'],['Goals','/goals'],['Assets','/assets'],['Shared groups','/groups']]},
  {label:'Preferences',items:[['Settings','/settings'],['Backups','/settings/backups']]}
];
export function hitPath(kind: SearchKind, id:string) {
  switch (kind) {
    case 'account': return '/accounts/'+id;
    case 'transaction': return '/transactions/'+id;
    case 'asset': return '/assets/'+id;
    case 'group': return '/groups/'+id;
    case 'category': return '/categories';
    case 'payee': return '/payees';
    case 'recurring': return '/recurring';
    case 'goal': return '/goals';
  }
}
