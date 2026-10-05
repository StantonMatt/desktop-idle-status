pragma ComponentBehavior: Bound
import QtQuick
import QtTest
import "../../plasmoid/contents/ui" as Widget

TestCase {
    id: test
    name: "StartCommand"
    Component { id: component; Widget.ServiceClient { function queryOwner() {} } }
    function test_pending_command_is_bounded_and_retry_launches() {
        const client = createTemporaryObject(component, test);
        client.startService();
        const first = client.serviceCommand;
        tryCompare(client, "startingService", false, 12000);
        compare(client.serviceStartFailed, true);
        client.startService();
        verify(first !== client.serviceCommand);
        tryCompare(client, "startingService", false, 12000);
        compare(client.serviceStartFailed, true);
    }
}
