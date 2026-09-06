Feature: fork task

  Scenario: parallel non-compete fork returns branch outputs in order
    Given a workflow with definition:
      """
      document: { dsl: '1.0.3', namespace: default, name: fork, version: '1.0.0' }
      do:
        - fanout:
            fork:
              compete: false
              branches:
                - a: { set: { value: 1 } }
                - b: { set: { value: 2 } }
                - c: { set: { value: 3 } }
      """
    When the workflow is executed
    Then the workflow should complete with output:
      """
      - value: 1
      - value: 2
      - value: 3
      """
