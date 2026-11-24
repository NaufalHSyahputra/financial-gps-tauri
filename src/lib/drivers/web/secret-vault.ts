type SecretBox = { nonce: Uint8Array<ArrayBuffer>; ciphertext: ArrayBuffer };

export async function openRecords(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open('financial-gps', 1);
    request.onupgradeneeded = () => request.result.createObjectStore('records');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
    request.onblocked = () => reject(new Error('Database upgrade blocked'));
  });
}

export class BrowserRecords {
  constructor(private readonly db: IDBDatabase) {}

  async get<T>(name: string): Promise<T | undefined> {
    return new Promise((resolve, reject) => {
      const tx = this.db.transaction('records', 'readonly');
      const request = tx.objectStore('records').get(name);
      tx.oncomplete = () => resolve(request.result as T | undefined);
      tx.onabort = tx.onerror = () => reject(tx.error);
    });
  }

  async set(name: string, value: unknown): Promise<void> {
    return this.write(store => { store.put(value, name); });
  }

  async remove(name: string): Promise<void> {
    return this.write(store => { store.delete(name); });
  }

  async write(operation: (store: IDBObjectStore) => void): Promise<void> {
    return new Promise((resolve, reject) => {
      const tx = this.db.transaction('records', 'readwrite');
      tx.oncomplete = () => resolve();
      tx.onabort = tx.onerror = () => reject(tx.error);
      operation(tx.objectStore('records'));
    });
  }

  async wrappingKey(): Promise<CryptoKey> {
    const generated = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']);
    let key = generated;
    await this.write(store => {
      const request = store.get('vault:key:v1');
      request.onsuccess = () => {
        if (request.result) key = request.result as CryptoKey;
        else store.add(generated, 'vault:key:v1');
      };
    });
    return key;
  }
}

const bytes = new TextEncoder();
const nameFor = (slot: string) => `vault:secret:${slot}`;
const additionalData = (slot: string) => bytes.encode(`financial-gps-vault:v1:${slot}`);

export class WebSecretVault {
  private key?: Promise<CryptoKey>;
  constructor(private readonly records: BrowserRecords) {}

  private getKey() {
    return this.key ??= this.records.wrappingKey();
  }

  async store(slot: string, contents: Uint8Array<ArrayBuffer>): Promise<void> {
    const key = await this.getKey();
    const nonce = crypto.getRandomValues(new Uint8Array(12));
    const ciphertext = await crypto.subtle.encrypt({ name: 'AES-GCM', iv: nonce, additionalData: additionalData(slot) }, key, contents);
    await this.records.set(nameFor(slot), { nonce, ciphertext } satisfies SecretBox);
  }

  async retrieve(slot: string): Promise<Uint8Array<ArrayBuffer> | undefined> {
    const box = await this.records.get<SecretBox>(nameFor(slot));
    if (!box) return;
    const key = await this.getKey();
    const plaintext = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: box.nonce, additionalData: additionalData(slot) }, key, box.ciphertext);
    return new Uint8Array(plaintext);
  }

  async revoke(slot: string): Promise<void> {
    await this.records.remove(nameFor(slot));
  }
}
