// What waits for the administrator: above the clients and above the machines alike.
import { words } from "../../client/src/i18n";
import { Icon } from "../../client/src/ui";
import { accessId, api, clientLabelOf, hostLabel, type State } from "./api";
import { ago, type Act } from "./common";

const T = words({ requests: "Requests", approve: "Approve", reject: "Reject" }, { requests: "Заявки", approve: "Одобрить", reject: "Отклонить" });

export function Requests({ s, act }: { s: State; act: Act }) {
  const requests = s.access.filter((a) => a.status === "pending");
  if (requests.length === 0) return null;
  const t = T();
  return (
    <section className="requests page">
      <div className="admin-head">
        <Icon name="shield" size={18} />
        <h2>{t.requests}</h2>
        <span className="count">{requests.length}</span>
      </div>
      <ul>
        {requests.map((a) => (
          <li key={accessId(a)} className="request">
            <div className="request-who selectable">
              <b title={clientLabelOf(s, a.client, a.client_name)}>{clientLabelOf(s, a.client, a.client_name)}</b>
              <span className="arrow">→</span>
              <b title={hostLabel(a)}>{hostLabel(a)}</b>
            </div>
            <div className="request-when selectable">
              ID {accessId(a)} · {ago(s.now, a.requested)}
            </div>
            <div className="request-actions">
              <button className="btn primary" onClick={() => act(() => api.approve(a.client, a.host))}>
                {t.approve}
              </button>
              <button className="btn danger" onClick={() => act(() => api.reject(a.client, a.host))}>
                {t.reject}
              </button>
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}
