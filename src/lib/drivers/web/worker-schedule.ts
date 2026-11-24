import type { BackupSchedule_Serialize, BackupSummary } from '../contracts';
import { asError, issue } from './errors';
import type { Opfs, SqliteDb } from './opfs';
import type { BrowserRecords, WebSecretVault } from './secret-vault';

type ScheduledLedger = { db: SqliteDb; entry: { ledgerInstanceId: string } };
const now = () => new Date().toISOString();
const decodeText = new TextDecoder();

export function backupSecretSlot(entry: ScheduledLedger['entry']) {
  return `backup:${entry.ledgerInstanceId}`;
}

export function getSchedule(active: ScheduledLedger): BackupSchedule_Serialize {
    const json = active.db.selectValue("SELECT value_json FROM ledger_settings WHERE key='backup.schedule'");
    return typeof json === 'string' ? JSON.parse(json) :
      { schedule: null, remembered: false, lastAttempt: null, lastSuccess: null, error: null, destinationName: null };
  }

export function writeSchedule(active: ScheduledLedger, status: BackupSchedule_Serialize) {
    active.db.exec({
      sql: "INSERT INTO ledger_settings VALUES('backup.schedule',?1) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
      bind: [JSON.stringify(status)]
    });
  }

export async function runSchedule<T extends ScheduledLedger>(active: T, {
  vault, records, opfs, createBackup, backupName, backups, persist
}: {
  vault: WebSecretVault; records: BrowserRecords; opfs: Opfs;
  createBackup: (active: T, password: string) => Promise<BackupSummary>;
  backupName: (entry: T['entry'], id: string) => string;
  backups: (entry: T['entry']) => Promise<BackupSummary[]>;
  persist: (active: T) => Promise<void>;
}) {
    const status = getSchedule(active);
    const schedule = status.schedule;
    if (!schedule) return;
    const timezone = active.db.selectValue('SELECT timezone_override FROM ledger_meta');
    const clock = new Intl.DateTimeFormat('sv', {
      year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit',
      hourCycle: 'h23', timeZone: typeof timezone === 'string' ? timezone : undefined
    });
    const [today, time] = clock.format(new Date()).split(' ');
    const date = new Date(`${today}T00:00:00Z`);
    const weekday = (date.getUTCDay() + 6) % 7 + 1;
    const days = schedule.weekday == null ? 0 : (weekday - schedule.weekday + 7) % 7;
    date.setUTCDate(date.getUTCDate() - days - (days === 0 && time < schedule.localTime ? schedule.weekday == null ? 1 : 7 : 0));
    const token = date.toISOString().slice(0, 10) + 'T' + schedule.localTime;
    if (active.db.selectValue("SELECT value_json FROM ledger_settings WHERE key='backup.browser_run'") === JSON.stringify(token)) return;
    status.lastAttempt = now();
    try {
      const secret = await vault.retrieve(backupSecretSlot(active.entry));
      if (!secret) throw issue('Validation', 'credential_required');
      const password = decodeText.decode(secret);
      secret.fill(0);
      const backup = await createBackup(active, password);
      const target = active.db.selectValue("SELECT value_json FROM ledger_settings WHERE key='backup.browser_destination'");
      const handle = typeof target === 'string' && JSON.parse(target) ? await records.get<FileSystemDirectoryHandle>(`backup-directory:${JSON.parse(target)}`) : undefined;
      if (handle) {
        const dest = await handle.getFileHandle(backup.fileName, { create: true });
        const out = await dest.createWritable();
        await out.write(await opfs.read(backupName(active.entry, backup.id)));
        await out.close();
      }
      status.lastSuccess = now();
      status.error = null;
      if (schedule.retainDays) {
        const cutoff = Date.now() - schedule.retainDays * 86400_000;
        const list = await backups(active.entry);
        const expired = list.filter(item => Date.parse(item.createdAt) < cutoff);
        for (const item of expired) await opfs.remove(backupName(active.entry, item.id)).catch(() => {});
        if (expired.length) await records.set(`backups:${active.entry.ledgerInstanceId}`, list.filter(item => !expired.includes(item)));
      }
    } catch (error) { status.error = asError(error).messageKey; }
    writeSchedule(active, status);
    active.db.exec({
      sql: "INSERT INTO ledger_settings VALUES('backup.browser_run',?1) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json",
      bind: [JSON.stringify(token)]
    });
    await persist(active);
  }
